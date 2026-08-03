use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    error::AppError,
    models::skills::{SkillPayload, SkillReview, UserSkillRecord, UserSkillResponse},
};

const COLUMNS: &str = "id,owner_user_id,name,description,skill_markdown,content_hash,version,enabled,created_at,updated_at";

pub async fn list(pool: &SqlitePool, user_id: &str) -> Result<Vec<UserSkillResponse>, AppError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM user_skills WHERE owner_user_id=? ORDER BY updated_at DESC,id DESC"
    );
    let records: Vec<UserSkillRecord> = sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_all(pool)
        .await
        .map_err(AppError::Database)?;
    let mut result = Vec::with_capacity(records.len());
    for record in records {
        result.push(with_review(pool, record).await?);
    }
    Ok(result)
}

pub async fn find_owned(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
) -> Result<Option<UserSkillRecord>, AppError> {
    let sql = format!("SELECT {COLUMNS} FROM user_skills WHERE id=? AND owner_user_id=?");
    sqlx::query_as(&sql)
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(AppError::Database)
}

pub async fn find_response(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
) -> Result<Option<UserSkillResponse>, AppError> {
    match find_owned(pool, user_id, id).await? {
        Some(record) => Ok(Some(with_review(pool, record).await?)),
        None => Ok(None),
    }
}

pub async fn create(
    pool: &SqlitePool,
    user_id: &str,
    payload: &SkillPayload,
    content_hash: &str,
) -> Result<UserSkillResponse, AppError> {
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO user_skills(id,owner_user_id,name,description,skill_markdown,content_hash,enabled) VALUES(?,?,?,?,?,?,?)")
        .bind(&id)
        .bind(user_id)
        .bind(payload.name.trim())
        .bind(payload.description.as_deref().map(str::trim).filter(|value| !value.is_empty()))
        .bind(&payload.skill_markdown)
        .bind(content_hash)
        .bind(payload.enabled)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    find_response(pool, user_id, &id)
        .await?
        .ok_or_else(|| AppError::Config("created Skill is missing".into()))
}

pub async fn update(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
    payload: &SkillPayload,
    content_hash: &str,
) -> Result<Option<UserSkillResponse>, AppError> {
    let Some(current) = find_owned(pool, user_id, id).await? else {
        return Ok(None);
    };
    let content_changed = current.content_hash != content_hash;
    let mut tx = pool.begin().await.map_err(AppError::Database)?;
    sqlx::query("UPDATE user_skills SET name=?,description=?,skill_markdown=?,content_hash=?,version=version+?,enabled=?,updated_at=CURRENT_TIMESTAMP WHERE id=? AND owner_user_id=?")
        .bind(payload.name.trim())
        .bind(payload.description.as_deref().map(str::trim).filter(|value| !value.is_empty()))
        .bind(&payload.skill_markdown)
        .bind(content_hash)
        .bind(i64::from(content_changed))
        .bind(payload.enabled)
        .bind(id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;
    if content_changed {
        sqlx::query("DELETE FROM skill_reviews WHERE skill_id=?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;
    }
    tx.commit().await.map_err(AppError::Database)?;
    find_response(pool, user_id, id).await
}

pub async fn delete(pool: &SqlitePool, user_id: &str, id: &str) -> Result<bool, AppError> {
    Ok(
        sqlx::query("DELETE FROM user_skills WHERE id=? AND owner_user_id=?")
            .bind(id)
            .bind(user_id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?
            .rows_affected()
            == 1,
    )
}

pub async fn save_review(
    pool: &SqlitePool,
    skill: &UserSkillRecord,
    reviewer_model: &str,
    review: &SkillReview,
) -> Result<bool, AppError> {
    let findings = serde_json::json!({
        "warnings": review.warnings,
        "suggestions": review.suggestions
    });
    let affected = sqlx::query("INSERT INTO skill_reviews(skill_id,skill_version,skill_content_hash,reviewer_model,rubric_version,overall_score,grade,dimension_scores_json,findings_json,evaluated_at) SELECT id,version,content_hash,?,?,?,?,?,?,CURRENT_TIMESTAMP FROM user_skills WHERE id=? AND owner_user_id=? AND version=? AND content_hash=? ON CONFLICT(skill_id) DO UPDATE SET skill_version=excluded.skill_version,skill_content_hash=excluded.skill_content_hash,reviewer_model=excluded.reviewer_model,rubric_version=excluded.rubric_version,overall_score=excluded.overall_score,grade=excluded.grade,dimension_scores_json=excluded.dimension_scores_json,findings_json=excluded.findings_json,evaluated_at=CURRENT_TIMESTAMP")
        .bind(reviewer_model)
        .bind("skill-quality-v1").bind(review.overall_score).bind(&review.grade)
        .bind(review.dimensions.to_string()).bind(findings.to_string())
        .bind(&skill.id).bind(&skill.owner_user_id).bind(skill.version).bind(&skill.content_hash)
        .execute(pool).await.map_err(AppError::Database)?.rows_affected();
    Ok(affected == 1)
}

async fn with_review(
    pool: &SqlitePool,
    record: UserSkillRecord,
) -> Result<UserSkillResponse, AppError> {
    let row: Option<(i64, String, String, String, String)> = sqlx::query_as("SELECT overall_score,grade,dimension_scores_json,findings_json,evaluated_at FROM skill_reviews WHERE skill_id=? AND skill_version=? AND skill_content_hash=?")
        .bind(&record.id).bind(record.version).bind(&record.content_hash)
        .fetch_optional(pool).await.map_err(AppError::Database)?;
    let review = row.and_then(
        |(overall_score, grade, dimensions, findings, evaluated_at)| {
            let findings: serde_json::Value = serde_json::from_str(&findings).ok()?;
            Some(SkillReview {
                overall_score,
                grade,
                dimensions: serde_json::from_str(&dimensions).ok()?,
                warnings: serde_json::from_value(findings.get("warnings")?.clone()).ok()?,
                suggestions: serde_json::from_value(findings.get("suggestions")?.clone()).ok()?,
                evaluated_at: Some(evaluated_at),
            })
        },
    );
    Ok(UserSkillResponse {
        id: record.id,
        name: record.name,
        description: record.description,
        skill_markdown: record.skill_markdown,
        content_hash: record.content_hash,
        version: record.version,
        enabled: record.enabled,
        created_at: record.created_at,
        updated_at: record.updated_at,
        review,
    })
}
