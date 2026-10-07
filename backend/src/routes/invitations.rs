use actix_web::{HttpRequest, HttpResponse, get, http::StatusCode, post, web};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, QueryBuilder, Sqlite};
use uuid::Uuid;

use crate::{AppState, auth::extractor::RequireAdmin, error::AppError};

#[derive(Deserialize)]
pub struct CreateInvitationsRequest {
    pub count: Option<i64>,
    /// Missing means seven days; explicit null means no expiry.
    #[serde(default, deserialize_with = "deserialize_validity_days")]
    pub validity_days: Option<Option<i64>>,
    pub note: Option<String>,
}

fn deserialize_validity_days<'de, D>(deserializer: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<i64>::deserialize(deserializer).map(Some)
}

#[derive(Serialize)]
struct CreatedInvitation {
    id: String,
    code: String,
    expires_at: Option<String>,
}

#[derive(Serialize)]
struct CreatedInvitationBatch {
    batch_id: String,
    invitations: Vec<CreatedInvitation>,
}

#[derive(Deserialize)]
pub struct InvitationListQuery {
    pub status: Option<String>,
    pub batch_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Serialize, FromRow)]
pub struct InvitationItem {
    pub id: String,
    pub batch_id: String,
    pub note: String,
    pub created_by_username: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub status: String,
    pub used_by_username: Option<String>,
    pub used_at: Option<String>,
}

#[derive(Serialize)]
struct InvitationPage {
    items: Vec<InvitationItem>,
    next_cursor: Option<String>,
}

fn bad_request(message: &'static str) -> AppError {
    AppError::api(
        StatusCode::BAD_REQUEST,
        "INVITATION_INVALID_REQUEST",
        message,
    )
}

fn encode_cursor(created_at: &str, id: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{created_at}|{id}"))
}

fn decode_cursor(cursor: Option<&str>) -> Result<Option<(String, String)>, AppError> {
    cursor
        .map(|value| {
            let raw = URL_SAFE_NO_PAD
                .decode(value)
                .map_err(|_| bad_request("cursor 无效"))?;
            let raw = String::from_utf8(raw).map_err(|_| bad_request("cursor 无效"))?;
            let (created_at, id) = raw
                .split_once('|')
                .ok_or_else(|| bad_request("cursor 无效"))?;
            Ok((created_at.to_owned(), id.to_owned()))
        })
        .transpose()
}

fn new_code() -> (String, String) {
    let mut random = [0_u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let raw = random
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    let canonical = format!("RAIN{raw}");
    let displayed = format!(
        "RAIN-{}-{}-{}-{}-{}",
        &raw[0..8],
        &raw[8..16],
        &raw[16..24],
        &raw[24..32],
        &raw[32..40]
    );
    let digest = Sha256::digest(canonical.as_bytes());
    (displayed, format!("{digest:x}"))
}

#[post("/admin/invitations")]
pub async fn create(
    admin: RequireAdmin,
    state: web::Data<AppState>,
    request: HttpRequest,
    body: web::Json<CreateInvitationsRequest>,
) -> Result<HttpResponse, AppError> {
    let count = body.count.unwrap_or(1);
    if !(1..=100).contains(&count) {
        return Err(bad_request("每批邀请码数量必须为 1 到 100"));
    }
    let validity_days = body.validity_days.unwrap_or(Some(7));
    if validity_days.is_some_and(|days| ![1, 7, 30].contains(&days)) {
        return Err(bad_request("有效期只能是 1、7、30 天或永久"));
    }
    let note = body.note.clone().unwrap_or_default();
    if note.chars().count() > 200 {
        return Err(bad_request("备注最多 200 个字符"));
    }

    let batch_id = Uuid::new_v4().to_string();
    let expires_at = validity_days.map(|days| (Utc::now() + Duration::days(days)).to_rfc3339());
    let values: Vec<(String, String, String)> = (0..count)
        .map(|_| {
            let (code, hash) = new_code();
            (Uuid::new_v4().to_string(), code, hash)
        })
        .collect();
    let insert_values = values
        .iter()
        .map(|(id, _, hash)| (id.clone(), hash.clone()))
        .collect::<Vec<_>>();
    let actor_id = admin.0.id.clone();
    let note_for_db = note.clone();
    let expires_for_db = expires_at.clone();
    let batch_for_db = batch_id.clone();
    let ip = request.peer_addr().map(|address| address.ip().to_string());
    let user_agent = request
        .headers()
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let audit_id = Uuid::new_v4().to_string();
    let operation_id = Uuid::new_v4().to_string();

    crate::db::write::transaction(&state.db.pool, "create invitations", move |conn| {
        let insert_values = insert_values.clone();
        let actor_id = actor_id.clone();
        let note = note_for_db.clone();
        let expires_at = expires_for_db.clone();
        let batch_id = batch_for_db.clone();
        let ip = ip.clone();
        let user_agent = user_agent.clone();
        let audit_id = audit_id.clone();
        let operation_id = operation_id.clone();
        Box::pin(async move {
            for (id, hash) in &insert_values {
                sqlx::query("INSERT INTO invitations(id,batch_id,code_hash,note,created_by,expires_at) VALUES(?,?,?,?,?,?)")
                    .bind(id)
                    .bind(&batch_id)
                    .bind(hash)
                    .bind(&note)
                    .bind(&actor_id)
                    .bind(expires_at.as_deref())
                    .execute(&mut *conn)
                    .await
                    .map_err(AppError::Database)?;
            }
            let details = serde_json::json!({"batch_id": batch_id, "count": insert_values.len(), "expires_at": expires_at}).to_string();
            sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,operation_id,details_json,client_ip,user_agent) VALUES(?,'USER',?,'INVITATION_CREATED',?,?,?,?)")
                .bind(&audit_id)
                .bind(&actor_id)
                .bind(&operation_id)
                .bind(details)
                .bind(ip.as_deref())
                .bind(user_agent.as_deref())
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
            Ok(())
        })
    })
    .await?;

    let response = CreatedInvitationBatch {
        batch_id,
        invitations: values
            .into_iter()
            .map(|(id, code, _)| CreatedInvitation {
                id,
                code,
                expires_at: expires_at.clone(),
            })
            .collect(),
    };
    Ok(HttpResponse::Created()
        .insert_header(("Cache-Control", "no-store"))
        .json(response))
}

#[get("/admin/invitations")]
pub async fn list(
    _admin: RequireAdmin,
    state: web::Data<AppState>,
    query: web::Query<InvitationListQuery>,
) -> Result<HttpResponse, AppError> {
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(bad_request("limit 必须为 1 到 100"));
    }
    let cursor = decode_cursor(query.cursor.as_deref())?;
    if query
        .batch_id
        .as_deref()
        .is_some_and(|value| value.len() > 64)
    {
        return Err(bad_request("batch_id 无效"));
    }
    if query
        .status
        .as_deref()
        .is_some_and(|status| !["ACTIVE", "USED", "REVOKED", "EXPIRED"].contains(&status))
    {
        return Err(bad_request("邀请码状态无效"));
    }

    let mut sql = QueryBuilder::<Sqlite>::new(
        "SELECT i.id,i.batch_id,i.note,creator.username AS created_by_username,i.created_at,i.expires_at,CASE WHEN i.used_at IS NOT NULL THEN 'USED' WHEN i.revoked_at IS NOT NULL THEN 'REVOKED' WHEN i.expires_at IS NOT NULL AND datetime(i.expires_at)<=CURRENT_TIMESTAMP THEN 'EXPIRED' ELSE 'ACTIVE' END AS status,user.username AS used_by_username,i.used_at FROM invitations i JOIN users creator ON creator.id=i.created_by LEFT JOIN users user ON user.id=i.used_by WHERE 1=1",
    );
    if let Some(status) = query.status.as_deref() {
        sql.push(" AND CASE WHEN i.used_at IS NOT NULL THEN 'USED' WHEN i.revoked_at IS NOT NULL THEN 'REVOKED' WHEN i.expires_at IS NOT NULL AND datetime(i.expires_at)<=CURRENT_TIMESTAMP THEN 'EXPIRED' ELSE 'ACTIVE' END = ")
            .push_bind(status);
    }
    if let Some(batch_id) = query.batch_id.as_deref() {
        sql.push(" AND i.batch_id=").push_bind(batch_id);
    }
    if let Some((created_at, id)) = cursor {
        sql.push(" AND (i.created_at,i.id)<(")
            .push_bind(created_at)
            .push(",")
            .push_bind(id)
            .push(")");
    }
    sql.push(" ORDER BY i.created_at DESC,i.id DESC LIMIT ")
        .push_bind(limit + 1);
    let mut items = sql
        .build_query_as::<InvitationItem>()
        .fetch_all(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
    let next_cursor = if items.len() as i64 > limit {
        items.pop();
        items
            .last()
            .map(|item| encode_cursor(&item.created_at, &item.id))
    } else {
        None
    };
    Ok(HttpResponse::Ok().json(InvitationPage { items, next_cursor }))
}

#[post("/admin/invitations/{invitation_id}/revoke")]
pub async fn revoke(
    admin: RequireAdmin,
    state: web::Data<AppState>,
    request: HttpRequest,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let invitation_id = path.into_inner();
    if invitation_id.len() > 64 {
        return Err(bad_request("邀请码编号无效"));
    }
    let actor_id = admin.0.id.clone();
    let ip = request.peer_addr().map(|address| address.ip().to_string());
    let user_agent = request
        .headers()
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let audit_id = Uuid::new_v4().to_string();
    let operation_id = Uuid::new_v4().to_string();
    let input = (
        invitation_id,
        actor_id,
        ip,
        user_agent,
        audit_id,
        operation_id,
    );
    crate::db::write::transaction(&state.db.pool, "revoke invitation", move |conn| {
        let input = input.clone();
        Box::pin(async move {
            let current = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
                "SELECT used_at,revoked_at,expires_at FROM invitations WHERE id=?",
            )
            .bind(&input.0)
            .fetch_optional(&mut *conn)
            .await
            .map_err(AppError::Database)?
            .ok_or_else(|| AppError::api(StatusCode::NOT_FOUND, "INVITATION_NOT_FOUND", "邀请码不存在"))?;
            if current.0.is_some() {
                return Err(AppError::api(StatusCode::CONFLICT, "INVITATION_NOT_REVOCABLE", "已使用、已撤销或已过期的邀请码不能撤销"));
            }
            if current.1.is_some() {
                return Ok(());
            }
            if current.2.as_deref().is_some_and(|value| {
                chrono::DateTime::parse_from_rfc3339(value)
                    .is_ok_and(|expires_at| expires_at <= Utc::now())
            }) {
                return Err(AppError::api(StatusCode::CONFLICT, "INVITATION_NOT_REVOCABLE", "已使用、已撤销或已过期的邀请码不能撤销"));
            }
            let changed = sqlx::query("UPDATE invitations SET revoked_at=CURRENT_TIMESTAMP,revoked_by=? WHERE id=? AND used_at IS NULL AND revoked_at IS NULL AND (expires_at IS NULL OR datetime(expires_at)>CURRENT_TIMESTAMP)")
                .bind(&input.1)
                .bind(&input.0)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?
                .rows_affected();
            if changed != 1 {
                return Err(AppError::api(StatusCode::CONFLICT, "INVITATION_NOT_REVOCABLE", "已使用、已撤销或已过期的邀请码不能撤销"));
            }
            let details = serde_json::json!({"invitation_id": input.0}).to_string();
            sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,operation_id,details_json,client_ip,user_agent) VALUES(?,'USER',?,'INVITATION_REVOKED',?,?,?,?)")
                .bind(&input.4)
                .bind(&input.1)
                .bind(&input.5)
                .bind(details)
                .bind(input.2.as_deref())
                .bind(input.3.as_deref())
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
            Ok(())
        })
    })
    .await?;
    Ok(HttpResponse::NoContent().finish())
}
