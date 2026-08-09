use std::sync::Arc;

use actix_web::{HttpMessage, HttpRequest, HttpResponse, get, http::StatusCode, post, web};
use async_stream::stream;
use serde::Deserialize;

use crate::{
    AppState, RequestLogId,
    ai_provider::{client::OpenAiChatClient, config::resolve_effective_config},
    auth::extractor::RequireBusinessUser,
    error::AppError,
    models::skill_runs::NewSkillRun,
    repositories::{skill_runs, skills},
    services::skill_runner::SkillRunner,
};

#[derive(Deserialize)]
pub struct CreateSkillRun {
    skill_id: String,
}

fn not_found() -> AppError {
    AppError::api(
        StatusCode::NOT_FOUND,
        "SKILL_RUN_NOT_FOUND",
        "Skill 任务不存在或已过期",
    )
}

#[post("/issues/{issue_code}/skill-runs")]
pub async fn create(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    issue_code: web::Path<String>,
    body: web::Json<CreateSkillRun>,
    request: HttpRequest,
) -> Result<HttpResponse, AppError> {
    let issue_code = issue_code.trim().to_ascii_uppercase();
    let issue_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM issues WHERE code=? AND status='ACTIVE')")
            .bind(&issue_code)
            .fetch_one(&state.db.pool)
            .await
            .map_err(AppError::Database)?;
    if !issue_exists {
        return Err(AppError::api(
            StatusCode::NOT_FOUND,
            "ISSUE_NOT_FOUND",
            "Issue 不存在",
        ));
    }
    let skill = skills::find_owned(&state.db.pool, &user.0.id, &body.skill_id)
        .await?
        .ok_or_else(|| AppError::api(StatusCode::NOT_FOUND, "SKILL_NOT_FOUND", "Skill 不存在"))?;
    if !skill.enabled {
        return Err(AppError::api(
            StatusCode::CONFLICT,
            "SKILL_DISABLED",
            "Skill 已停用",
        ));
    }
    let provider = resolve_effective_config(&state.db.pool, &state.ai_provider)
        .await?
        .ok_or_else(|| {
            AppError::api(
                StatusCode::CONFLICT,
                "AI_PROVIDER_NOT_CONFIGURED",
                "模型服务尚未配置",
            )
        })?;
    let client = OpenAiChatClient::new(&provider).map_err(|_| {
        AppError::api(
            StatusCode::BAD_GATEWAY,
            "AI_PROVIDER_UNAVAILABLE",
            "模型服务不可用",
        )
    })?;
    let run = skill_runs::create(
        &state.db.pool,
        &NewSkillRun {
            user_id: user.0.id.clone(),
            issue_code,
            skill_id: skill.id,
            skill_version: skill.version,
            skill_name: skill.name,
            skill_snapshot_markdown: skill.skill_markdown,
        },
    )
    .await
    .map_err(|error| {
        if matches!(&error, AppError::Database(sqlx::Error::Database(db)) if db.is_unique_violation())
        {
            AppError::api(
                StatusCode::CONFLICT,
                "SKILL_RUN_ALREADY_ACTIVE",
                "当前用户已有正在运行的 Skill 任务",
            )
        } else {
            error
        }
    })?;
    let (cancellation, _) = state.skill_runs.register(&run.id);
    let request_id = request
        .extensions()
        .get::<RequestLogId>()
        .map(|value| value.0.clone());
    tracing::info!(
        request_id = request_id.as_deref().unwrap_or("unavailable"),
        run_id = %run.id,
        skill_id = %run.skill_id,
        skill_version = run.skill_version,
        "skill run queued"
    );
    let run_id = run.id.clone();
    let runner_state = state.clone();
    actix_web::rt::spawn(async move {
        SkillRunner::execute(runner_state, run_id, Arc::new(client), cancellation).await;
    });
    Ok(HttpResponse::Accepted().json(run))
}

#[get("/skill-runs/{id}")]
pub async fn get_run(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let run = skill_runs::find_owned(&state.db.pool, &id, &user.0.id)
        .await?
        .ok_or_else(not_found)?;
    Ok(HttpResponse::Ok().json(run))
}

#[get("/me/skill-runs/active")]
pub async fn get_active_run(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(skill_runs::find_active_owned(&state.db.pool, &user.0.id).await?))
}

#[post("/skill-runs/{id}/cancel")]
pub async fn cancel(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    if skill_runs::cancel(&state.db.pool, &id, &user.0.id).await? {
        state.skill_runs.cancel(&id);
        state.skill_runs.emit(
            &id,
            crate::SkillRunEvent {
                event: "run.cancelled".into(),
                data: serde_json::json!({}),
            },
        );
    } else if skill_runs::find_owned(&state.db.pool, &id, &user.0.id)
        .await?
        .is_none()
    {
        return Err(not_found());
    }
    let run = skill_runs::find_owned(&state.db.pool, &id, &user.0.id)
        .await?
        .ok_or_else(not_found)?;
    Ok(HttpResponse::Ok().json(run))
}

#[get("/skill-runs/{id}/result")]
pub async fn result(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let run = skill_runs::find_owned(&state.db.pool, &id, &user.0.id)
        .await?
        .ok_or_else(not_found)?;
    if run.status != "SUCCEEDED" {
        return Err(AppError::api(
            StatusCode::CONFLICT,
            "SKILL_RUN_NOT_COMPLETE",
            "Skill 任务尚未完成",
        ));
    }
    let value: serde_json::Value =
        serde_json::from_str(run.result_json.as_deref().unwrap_or(""))
            .map_err(|_| AppError::Config("stored Skill result is invalid".into()))?;
    Ok(HttpResponse::Ok().json(value))
}

#[get("/skill-runs/{id}/events")]
pub async fn events(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let run = skill_runs::find_owned(&state.db.pool, &id, &user.0.id)
        .await?
        .ok_or_else(not_found)?;
    let mut receiver = state.skill_runs.subscribe(&id);
    let initial = serde_json::to_string(&run)
        .map_err(|_| AppError::Config("failed to serialize Skill run".into()))?;
    let body = stream! {
        yield Ok::<_, actix_web::Error>(web::Bytes::from(format!("event: snapshot\ndata: {initial}\n\n")));
        if let Some(ref mut receiver) = receiver {
            let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
            heartbeat.tick().await;
            loop {
                tokio::select! {
                    _ = heartbeat.tick() => {
                        yield Ok(web::Bytes::from_static(b": heartbeat\n\n"));
                    }
                    received = receiver.recv() => match received {
                        Ok(event) => {
                            let data = serde_json::to_string(&event.data).unwrap_or_else(|_| "{}".into());
                            yield Ok(web::Bytes::from(format!("event: {}\ndata: {data}\n\n", event.event)));
                            if matches!(event.event.as_str(), "run.completed" | "run.failed" | "run.cancelled") { break; }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    };
    Ok(HttpResponse::Ok()
        .insert_header(("content-type", "text/event-stream"))
        .insert_header(("cache-control", "no-store"))
        .streaming(body))
}
