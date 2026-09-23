use std::time::Duration;

use actix_web::{HttpRequest, HttpResponse, get, http::StatusCode, post, put, web};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    AppState,
    ai_provider::{
        client::{ChatMessage, ChatRequest, OpenAiChatClient, ProviderError},
        config::{ProviderSource, ResolvedAiProvider, resolve_effective_config},
        crypto::SecretCipher,
        observability::ProviderRequestContext,
        retry::complete_with_retry_until,
    },
    auth::extractor::{RequireAdmin, RequireBusinessUser},
    config::StructuredOutputMode,
    error::AppError,
};

#[derive(Debug, Deserialize)]
pub struct UpdateAiProvider {
    expected_revision: Option<serde_json::Value>,
    base_url: String,
    api_key: Option<String>,
    model: String,
    request_timeout_seconds: u64,
    structured_output: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestAiProvider {
    base_url: String,
    api_key: String,
    model: String,
    request_timeout_seconds: u64,
}

async fn provider_snapshot(state: &AppState) -> Result<serde_json::Value, AppError> {
    let resolved = resolve_effective_config(&state.db.pool, &state.ai_provider).await?;
    let revision: Option<i64> =
        sqlx::query_scalar("SELECT provider_revision FROM ai_provider_settings WHERE id=1")
            .fetch_optional(&state.db.pool)
            .await
            .map_err(AppError::Database)?;
    Ok(match resolved {
        Some(provider) => serde_json::json!({
            "configured": true,
            "revision": revision.map(|value| value.to_string()),
            "source": provider.source,
            "base_url": provider.base_url,
            "model": provider.model,
            "request_timeout_seconds": provider.timeout_seconds,
            "structured_output": provider.structured_output.as_str(),
            "api_key_mask": "••••••••",
        }),
        None => serde_json::json!({
            "configured": false,
            "revision": revision.map(|value| value.to_string()),
            "source": null,
            "base_url": null,
            "model": null,
            "request_timeout_seconds": state.ai_provider.timeout_seconds,
            "structured_output": state.ai_provider.structured_output.as_str(),
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

    let existing: Option<(String, String, String, i64, Option<String>, i64)> = sqlx::query_as(
        "SELECT base_url,encrypted_api_key,model,request_timeout_seconds,structured_output,provider_revision FROM ai_provider_settings WHERE id=1",
    )
    .fetch_optional(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    let expected_revision = body
        .expected_revision
        .as_ref()
        .map(parse_provider_revision)
        .transpose()?;
    let current_revision = existing.as_ref().map(|row| row.5).unwrap_or(0);
    if expected_revision.is_some_and(|revision| revision != current_revision) {
        return Err(AppError::public(
            StatusCode::CONFLICT,
            "AI_PROVIDER_REVISION_CONFLICT",
            format!("模型服务配置已更新，请刷新后重试（当前版本 {current_revision}）"),
        ));
    }
    let structured_output = body
        .structured_output
        .as_deref()
        .or_else(|| existing.as_ref().and_then(|row| row.4.as_deref()))
        .map(|value| StructuredOutputMode::parse(Some(value)))
        .transpose()?
        .unwrap_or(state.ai_provider.structured_output);
    let replacement_key = body
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty());
    if replacement_key.is_none()
        && existing
            .as_ref()
            .is_some_and(|row| row.0.trim_end_matches('/') != base_url)
    {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "AI_API_KEY_REQUIRED_FOR_BASE_URL_CHANGE",
            "修改 Base URL 时必须重新输入 API Key",
        ));
    }
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
        let existing = existing.as_ref().ok_or_else(|| {
            AppError::api(
                StatusCode::BAD_REQUEST,
                "AI_API_KEY_REQUIRED",
                "首次保存模型服务时必须提供 API Key",
            )
        })?;
        let master_key = state.ai_provider.master_key.ok_or_else(|| {
            AppError::api(
                StatusCode::CONFLICT,
                "AI_MASTER_KEY_REQUIRED",
                "复用已保存的 API Key 前必须配置 RAIN_AI_MASTER_KEY",
            )
        })?;
        SecretCipher::new(master_key)
            .decrypt(&existing.1)
            .map_err(|_| {
                AppError::api(
                    StatusCode::CONFLICT,
                    "AI_MASTER_KEY_INVALID",
                    "无法解密已保存的 API Key，请配置正确的 RAIN_AI_MASTER_KEY 或重新输入 API Key",
                )
            })?;
        existing.1.clone()
    };

    let mut tx = state.db.pool.begin().await.map_err(AppError::Database)?;
    sqlx::query(
        "INSERT INTO ai_provider_settings(id,base_url,encrypted_api_key,model,request_timeout_seconds,structured_output,updated_by_user_id,updated_at) VALUES(1,?,?,?,?,?,?,CURRENT_TIMESTAMP) ON CONFLICT(id) DO UPDATE SET base_url=excluded.base_url,encrypted_api_key=excluded.encrypted_api_key,model=excluded.model,request_timeout_seconds=excluded.request_timeout_seconds,structured_output=excluded.structured_output,updated_by_user_id=excluded.updated_by_user_id,updated_at=CURRENT_TIMESTAMP,provider_revision=provider_revision+1 WHERE provider_revision=?",
    )
    .bind(&base_url)
    .bind(&encrypted_api_key)
    .bind(model)
    .bind(body.request_timeout_seconds as i64)
    .bind(structured_output.as_str())
    .bind(&admin.0.id)
    .bind(current_revision)
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;
    if existing.is_some()
        && sqlx::query_scalar::<_, i64>(
            "SELECT provider_revision FROM ai_provider_settings WHERE id=1",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(AppError::Database)?
            == current_revision
    {
        return Err(AppError::public(
            StatusCode::CONFLICT,
            "AI_PROVIDER_REVISION_CONFLICT",
            "模型服务配置已被其他管理员更新，请刷新后重试",
        ));
    }
    let old_value = existing.as_ref().map(|row| {
        serde_json::json!({
            "base_url": row.0,
            "model": row.2,
            "request_timeout_seconds": row.3,
            "structured_output": row.4,
            "api_key_configured": true
        })
        .to_string()
    });
    let new_value = serde_json::json!({
        "base_url": base_url,
        "model": model,
        "request_timeout_seconds": body.request_timeout_seconds,
        "structured_output": structured_output.as_str(),
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

fn parse_provider_revision(value: &serde_json::Value) -> Result<i64, AppError> {
    let revision = match value {
        serde_json::Value::String(value) => value.parse::<i64>().ok(),
        serde_json::Value::Number(value) => value.as_i64(),
        _ => None,
    };
    revision.filter(|revision| *revision >= 0).ok_or_else(|| {
        AppError::api(
            StatusCode::BAD_REQUEST,
            "AI_PROVIDER_INVALID_REQUEST",
            "expected_revision 必须是非负十进制整数",
        )
    })
}

#[post("/admin/ai-provider/test")]
pub async fn test_ai_provider(
    req: HttpRequest,
    admin: RequireAdmin,
    state: web::Data<AppState>,
    body: web::Bytes,
) -> Result<HttpResponse, AppError> {
    let provider = if body.is_empty() {
        resolve_effective_config(&state.db.pool, &state.ai_provider)
            .await?
            .ok_or_else(|| {
                AppError::api(
                    StatusCode::CONFLICT,
                    "AI_PROVIDER_NOT_CONFIGURED",
                    "模型服务尚未配置",
                )
            })?
    } else {
        let value: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
            AppError::api(
                StatusCode::BAD_REQUEST,
                "INVALID_AI_PROVIDER_TEST",
                "模型服务测试配置无效",
            )
        })?;
        let object = value.as_object().ok_or_else(|| {
            AppError::api(
                StatusCode::BAD_REQUEST,
                "INVALID_AI_PROVIDER_TEST",
                "模型服务测试配置无效",
            )
        })?;
        const FIELDS: [&str; 4] = ["base_url", "api_key", "model", "request_timeout_seconds"];
        if object.keys().any(|key| !FIELDS.contains(&key.as_str())) {
            return Err(AppError::api(
                StatusCode::BAD_REQUEST,
                "INVALID_AI_PROVIDER_TEST",
                "模型服务测试配置无效",
            ));
        }
        if FIELDS.iter().any(|field| !object.contains_key(*field)) {
            return Err(AppError::api(
                StatusCode::BAD_REQUEST,
                "AI_PROVIDER_TEST_REQUIRES_COMPLETE_CONFIG",
                "测试新配置必须完整提供 Base URL、API Key、模型和超时",
            ));
        }
        let candidate: TestAiProvider = serde_json::from_value(value).map_err(|_| {
            AppError::api(
                StatusCode::BAD_REQUEST,
                "INVALID_AI_PROVIDER_TEST",
                "模型服务测试配置无效",
            )
        })?;
        let base_url = candidate.base_url.trim().trim_end_matches('/').to_owned();
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
        let api_key = candidate.api_key.trim();
        if api_key.is_empty() {
            return Err(AppError::api(
                StatusCode::BAD_REQUEST,
                "AI_API_KEY_REQUIRED",
                "测试新配置必须提供 API Key",
            ));
        }
        let model = candidate.model.trim();
        if model.is_empty() || model.len() > 200 {
            return Err(AppError::api(
                StatusCode::BAD_REQUEST,
                "INVALID_AI_MODEL",
                "模型名称不能为空且不能超过 200 个字符",
            ));
        }
        if !(1..=300).contains(&candidate.request_timeout_seconds) {
            return Err(AppError::api(
                StatusCode::BAD_REQUEST,
                "INVALID_AI_TIMEOUT",
                "请求超时必须为 1 到 300 秒",
            ));
        }
        ResolvedAiProvider::candidate(
            ProviderSource::Database,
            base_url,
            api_key.to_owned(),
            model.to_owned(),
            candidate.request_timeout_seconds,
        )
    };
    let base_url = provider.base_url.clone();
    let model = provider.model.clone();
    let timeout_seconds = provider.timeout_seconds;
    let client = OpenAiChatClient::new(&provider).map_err(provider_error)?;
    let context = ProviderRequestContext::provider_test(0);
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_seconds);
    let outcome = complete_with_retry_until(
        &client,
        ChatRequest {
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
        },
        context,
        deadline,
    )
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
