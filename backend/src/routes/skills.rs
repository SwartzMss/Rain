use actix_web::{HttpResponse, delete, get, http::StatusCode, post, put, web};
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    ai_provider::{
        client::{ChatCompletionClient, ChatMessage, ChatRequest, OpenAiChatClient},
        config::resolve_effective_config,
    },
    auth::extractor::RequireBusinessUser,
    error::AppError,
    models::skills::{SkillPayload, SkillReview},
    repositories::skills,
};

const MAX_SKILL_MARKDOWN_BYTES: usize = 64 * 1024;

fn validate(payload: &SkillPayload) -> Result<String, AppError> {
    let name = payload.name.trim();
    let description_len = payload
        .description
        .as_deref()
        .map(str::trim)
        .map(str::chars)
        .map(Iterator::count)
        .unwrap_or(0);
    if name.is_empty()
        || name.chars().count() > 100
        || description_len > 1000
        || payload.skill_markdown.trim().is_empty()
        || payload.skill_markdown.len() > MAX_SKILL_MARKDOWN_BYTES
    {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "SKILL_INVALID",
            "Skill 名称、描述或 SKILL.md 不符合格式要求",
        ));
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(payload.skill_markdown.as_bytes())
    ))
}

fn map_database_error(error: AppError) -> AppError {
    if matches!(&error, AppError::Database(sqlx::Error::Database(db)) if db.is_unique_violation()) {
        AppError::api(
            StatusCode::CONFLICT,
            "SKILL_NAME_CONFLICT",
            "已存在同名 Skill",
        )
    } else {
        error
    }
}

fn not_found() -> AppError {
    AppError::api(StatusCode::NOT_FOUND, "SKILL_NOT_FOUND", "Skill 不存在")
}

#[get("/me/skills")]
pub async fn list(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(skills::list(&state.db.pool, &user.0.id).await?))
}

#[get("/me/skills/{id}")]
pub async fn get(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let item = skills::find_response(&state.db.pool, &user.0.id, &id)
        .await?
        .ok_or_else(not_found)?;
    Ok(HttpResponse::Ok().json(item))
}

#[post("/me/skills")]
pub async fn create(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    payload: web::Json<SkillPayload>,
) -> Result<HttpResponse, AppError> {
    let hash = validate(&payload)?;
    let item = skills::create(&state.db.pool, &user.0.id, &payload, &hash)
        .await
        .map_err(map_database_error)?;
    Ok(HttpResponse::Created().json(item))
}

#[put("/me/skills/{id}")]
pub async fn update(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    id: web::Path<String>,
    payload: web::Json<SkillPayload>,
) -> Result<HttpResponse, AppError> {
    let hash = validate(&payload)?;
    let item = skills::update(&state.db.pool, &user.0.id, &id, &payload, &hash)
        .await
        .map_err(map_database_error)?
        .ok_or_else(not_found)?;
    Ok(HttpResponse::Ok().json(item))
}

#[delete("/me/skills/{id}")]
pub async fn delete_skill(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    if !skills::delete(&state.db.pool, &user.0.id, &id).await? {
        return Err(not_found());
    }
    Ok(HttpResponse::NoContent().finish())
}

#[post("/me/skills/{id}/review")]
pub async fn review(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let skill = skills::find_owned(&state.db.pool, &user.0.id, &id)
        .await?
        .ok_or_else(not_found)?;
    let provider = resolve_effective_config(&state.db.pool, &state.ai_provider)
        .await?
        .ok_or_else(|| {
            AppError::api(
                StatusCode::CONFLICT,
                "AI_PROVIDER_NOT_CONFIGURED",
                "模型服务尚未配置",
            )
        })?;
    let client = OpenAiChatClient::new(&provider).map_err(|_| review_failed())?;
    let prompt = format!(
        "Evaluate only the following SKILL.md. Score these dimensions with weights 20,25,20,15,10,10: task scope, retrieval strategy, evidence constraints, incomplete logs, stopping conditions, clarity. Unsupported shell, network, writes, SQL, scripts, cross-Issue access, or extra tools must be warnings. Return JSON with overall_score (0-100), grade, dimensions object, warnings string array, suggestions string array.\n\n<untrusted-skill-markdown>\n{}\n</untrusted-skill-markdown>",
        skill.skill_markdown
    );
    let request = ChatRequest {
        model: provider.model.clone(),
        messages: vec![ChatMessage {
            role: "system".into(),
            content: Some(prompt),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }],
        tools: Vec::new(),
        tool_choice: None,
        response_format: Some(serde_json::json!({"type":"json_object"})),
    };
    let first = client
        .complete(request.clone())
        .await
        .map_err(|_| review_failed())?;
    let review = match parse_review(first.message.content.as_deref()) {
        Ok(review) => review,
        Err(_) => {
            let mut repair = request;
            repair.messages.push(first.message);
            repair.messages.push(ChatMessage {
                role: "user".into(),
                content: Some(
                    "Return only valid JSON matching the requested review schema.".into(),
                ),
                tool_calls: Vec::new(),
                tool_call_id: None,
                name: None,
            });
            let repaired = client.complete(repair).await.map_err(|_| review_failed())?;
            parse_review(repaired.message.content.as_deref()).map_err(|_| review_failed())?
        }
    };
    if !skills::save_review(&state.db.pool, &skill, &provider.model, &review).await? {
        return Err(AppError::api(
            StatusCode::CONFLICT,
            "SKILL_CHANGED_DURING_REVIEW",
            "Skill 在评估期间发生变化，请重新评估",
        ));
    }
    let item = skills::find_response(&state.db.pool, &user.0.id, &id)
        .await?
        .ok_or_else(not_found)?;
    Ok(HttpResponse::Ok().json(item.review))
}

fn parse_review(content: Option<&str>) -> Result<SkillReview, ()> {
    let parsed: SkillReview = serde_json::from_str(content.ok_or(())?).map_err(|_| ())?;
    if !(0..=100).contains(&parsed.overall_score)
        || parsed.grade.trim().is_empty()
        || !parsed.dimensions.is_object()
    {
        return Err(());
    }
    Ok(parsed)
}

fn review_failed() -> AppError {
    AppError::api(
        StatusCode::BAD_GATEWAY,
        "SKILL_REVIEW_FAILED",
        "Skill 质量评估失败",
    )
}
