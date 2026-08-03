use actix_web::{HttpRequest, HttpResponse, get, http::StatusCode, post, put, web};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    AppState,
    ai_provider::{
        client::{ChatCompletionClient, ChatMessage, ChatRequest, OpenAiChatClient, ProviderError},
        config::{ProviderSource, ResolvedAiProvider, resolve_effective_config},
        crypto::SecretCipher,
    },
    auth::extractor::{RequireAdmin, RequireBusinessUser},
    error::AppError,
};

#[derive(Debug, Deserialize)]
pub struct UpdateAiProvider {
    base_url: String,
    api_key: Option<String>,
    model: String,
    request_timeout_seconds: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestAiProvider {
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    request_timeout_seconds: Option<u64>,
}

async fn provider_snapshot(state: &AppState) -> Result<serde_json::Value, AppError> {
    let resolved = resolve_effective_config(&state.db.pool, &state.ai_provider).await?;
    Ok(match resolved {
        Some(provider) => serde_json::json!({
            "configured": true,
            "source": provider.source,
            "base_url": provider.base_url,
            "model": provider.model,
            "request_timeout_seconds": provider.timeout_seconds,
            "api_key_mask": "••••••••",
        }),
        None => serde_json::json!({
            "configured": false,
            "source": null,
            "base_url": null,
            "model": null,
            "request_timeout_seconds": state.ai_provider.timeout_seconds,
            "api_key_mask": null,
        }),
    })
}

#[get("/admin/ai-provider")]
pub async fn get_ai_provider(
    _admin: RequireAdmin,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(provider_snapshot(&state).await?))
}

#[get("/me/ai-provider-status")]
pub async fn get_ai_provider_status(
    _user: RequireBusinessUser,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let configured = resolve_effective_config(&state.db.pool, &state.ai_provider)
        .await?
        .is_some();
    Ok(HttpResponse::Ok().json(serde_json::json!({"configured": configured})))
}

#[put("/admin/ai-provider")]
pub async fn update_ai_provider(
    req: HttpRequest,
    admin: RequireAdmin,
    state: web::Data<AppState>,
    body: web::Json<UpdateAiProvider>,
) -> Result<HttpResponse, AppError> {
    let base_url = body.base_url.trim().trim_end_matches('/').to_owned();
    let parsed = reqwest::Url::parse(&base_url).map_err(|_| invalid_base_url())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(invalid_base_url());
    }
    let model = body.model.trim();
    if model.is_empty() || model.len() > 200 {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "INVALID_AI_MODEL",
            "模型名称不能为空且不能超过 200 个字符",
        ));
    }
    if !(1..=300).contains(&body.request_timeout_seconds) {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "INVALID_AI_TIMEOUT",
            "请求超时必须为 1 到 300 秒",
        ));
    }

    let existing: Option<(String, String, String, i64)> = sqlx::query_as(
        "SELECT base_url,encrypted_api_key,model,request_timeout_seconds FROM ai_provider_settings WHERE id=1",
    )
    .fetch_optional(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    let replacement_key = body
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty());
    let encrypted_api_key = if let Some(api_key) = replacement_key {
        let master_key = state.ai_provider.master_key.ok_or_else(|| {
            AppError::api(
                StatusCode::CONFLICT,
                "AI_MASTER_KEY_REQUIRED",
                "保存 API Key 前必须配置 RAIN_AI_MASTER_KEY",
            )
        })?;
        SecretCipher::new(master_key).encrypt(api_key)?
    } else {
        existing.as_ref().map(|row| row.1.clone()).ok_or_else(|| {
            AppError::api(
                StatusCode::BAD_REQUEST,
                "AI_API_KEY_REQUIRED",
                "首次保存模型服务时必须提供 API Key",
            )
        })?
    };

    let mut tx = state.db.pool.begin().await.map_err(AppError::Database)?;
    sqlx::query(
        "INSERT INTO ai_provider_settings(id,base_url,encrypted_api_key,model,request_timeout_seconds,updated_by_user_id,updated_at) VALUES(1,?,?,?,?,?,CURRENT_TIMESTAMP) ON CONFLICT(id) DO UPDATE SET base_url=excluded.base_url,encrypted_api_key=excluded.encrypted_api_key,model=excluded.model,request_timeout_seconds=excluded.request_timeout_seconds,updated_by_user_id=excluded.updated_by_user_id,updated_at=CURRENT_TIMESTAMP",
    )
    .bind(&base_url)
    .bind(&encrypted_api_key)
    .bind(model)
    .bind(body.request_timeout_seconds as i64)
    .bind(&admin.0.id)
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;
    let old_value = existing.as_ref().map(|row| {
        serde_json::json!({
            "base_url": row.0,
            "model": row.2,
            "request_timeout_seconds": row.3,
            "api_key_configured": true
        })
        .to_string()
    });
    let new_value = serde_json::json!({
        "base_url": base_url,
        "model": model,
        "request_timeout_seconds": body.request_timeout_seconds,
        "api_key_configured": true,
        "api_key_replaced": replacement_key.is_some()
    })
    .to_string();
    sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,old_value,new_value,client_ip,user_agent) VALUES(?,'USER',?,'AI_PROVIDER_UPDATED',?,?,?,?)")
        .bind(Uuid::new_v4().to_string())
        .bind(&admin.0.id)
        .bind(old_value)
        .bind(new_value)
        .bind(req.peer_addr().map(|address| address.ip().to_string()))
        .bind(req.headers().get("user-agent").and_then(|value| value.to_str().ok()))
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;
    tx.commit().await.map_err(AppError::Database)?;

    Ok(HttpResponse::Ok().json(provider_snapshot(&state).await?))
}

fn invalid_base_url() -> AppError {
    AppError::api(
        StatusCode::BAD_REQUEST,
        "INVALID_AI_BASE_URL",
        "Base URL 必须是有效的 HTTP 或 HTTPS 地址",
    )
}

#[post("/admin/ai-provider/test")]
pub async fn test_ai_provider(
    req: HttpRequest,
    admin: RequireAdmin,
    state: web::Data<AppState>,
    body: web::Bytes,
) -> Result<HttpResponse, AppError> {
    let current = resolve_effective_config(&state.db.pool, &state.ai_provider).await?;
    let candidate = if body.is_empty() {
        TestAiProvider::default()
    } else {
        serde_json::from_slice(&body).map_err(|_| {
            AppError::api(
                StatusCode::BAD_REQUEST,
                "INVALID_AI_PROVIDER_TEST",
                "模型服务测试配置无效",
            )
        })?
    };
    let base_url = candidate
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| current.as_ref().map(|value| value.base_url.clone()))
        .ok_or_else(|| {
            AppError::api(
                StatusCode::CONFLICT,
                "AI_PROVIDER_NOT_CONFIGURED",
                "模型服务尚未配置",
            )
        })?;
    let parsed = reqwest::Url::parse(&base_url).map_err(|_| invalid_base_url())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(invalid_base_url());
    }
    let api_key = candidate
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| current.as_ref().map(|value| value.api_key().to_owned()))
        .ok_or_else(|| {
            AppError::api(
                StatusCode::CONFLICT,
                "AI_PROVIDER_NOT_CONFIGURED",
                "模型服务尚未配置",
            )
        })?;
    let model = candidate
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| current.as_ref().map(|value| value.model.clone()))
        .ok_or_else(|| {
            AppError::api(
                StatusCode::CONFLICT,
                "AI_PROVIDER_NOT_CONFIGURED",
                "模型服务尚未配置",
            )
        })?;
    if model.len() > 200 {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "INVALID_AI_MODEL",
            "模型名称不能为空且不能超过 200 个字符",
        ));
    }
    let timeout_seconds = candidate
        .request_timeout_seconds
        .or_else(|| current.as_ref().map(|value| value.timeout_seconds))
        .unwrap_or(state.ai_provider.timeout_seconds);
    if !(1..=300).contains(&timeout_seconds) {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "INVALID_AI_TIMEOUT",
            "请求超时必须为 1 到 300 秒",
        ));
    }
    let provider = ResolvedAiProvider::candidate(
        current
            .as_ref()
            .map_or(ProviderSource::Database, |value| value.source),
        base_url.clone(),
        api_key,
        model.clone(),
        timeout_seconds,
    );
    let client = OpenAiChatClient::new(&provider).map_err(provider_error)?;
    let outcome = client
        .complete(ChatRequest {
            model: model.clone(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: Some("Reply with the single word OK.".into()),
                tool_calls: Vec::new(),
                tool_call_id: None,
                name: None,
            }],
            tools: Vec::new(),
            tool_choice: None,
            response_format: None,
        })
        .await;
    let audit_value = serde_json::json!({
        "base_url": base_url,
        "model": model,
        "request_timeout_seconds": timeout_seconds,
        "ok": outcome.is_ok(),
    })
    .to_string();
    sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,new_value,client_ip,user_agent) VALUES(?,'USER',?,'AI_PROVIDER_TESTED',?,?,?)")
        .bind(Uuid::new_v4().to_string()).bind(&admin.0.id).bind(audit_value)
        .bind(req.peer_addr().map(|address| address.ip().to_string()))
        .bind(req.headers().get("user-agent").and_then(|value| value.to_str().ok()))
        .execute(&state.db.pool).await.map_err(AppError::Database)?;
    outcome.map_err(provider_error)?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true, "model": model })))
}

fn provider_error(error: ProviderError) -> AppError {
    match error {
        ProviderError::Timeout => AppError::api(
            StatusCode::GATEWAY_TIMEOUT,
            "AI_PROVIDER_TIMEOUT",
            "模型服务请求超时",
        ),
        _ => AppError::api(
            StatusCode::BAD_GATEWAY,
            error.code(),
            "模型服务连接测试失败",
        ),
    }
}
