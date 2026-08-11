use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use crate::{
    error::AppError,
    models::skills::{
        SkillPayload, SkillReview, UserSkillRecord, UserSkillResponse, UserSkillSummaryResponse,
    },
};

const COLUMNS: &str = "id,owner_user_id,name,description,skill_markdown,content_hash,version,enabled,created_at,updated_at";
pub const MAX_SKILLS_PER_USER: i64 = 50;

#[derive(FromRow)]
struct SkillListRow {
    id: String,
    name: String,
    description: Option<String>,
    skill_markdown: String,
    content_hash: String,
    version: i64,
    enabled: bool,
    created_at: String,
    updated_at: String,
    review_overall_score: Option<i64>,
    review_grade: Option<String>,
    review_dimensions: Option<String>,
    review_findings: Option<String>,
    review_evaluated_at: Option<String>,
}

pub async fn list(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<UserSkillSummaryResponse>, AppError> {
    let rows: Vec<SkillListRow> = sqlx::query_as(
        "SELECT s.id,s.name,s.description,s.skill_markdown,s.content_hash,s.version,s.enabled,s.created_at,s.updated_at,r.overall_score AS review_overall_score,r.grade AS review_grade,r.dimension_scores_json AS review_dimensions,r.findings_json AS review_findings,r.evaluated_at AS review_evaluated_at FROM user_skills s LEFT JOIN skill_reviews r ON r.skill_id=s.id AND r.skill_version=s.version AND r.skill_content_hash=s.content_hash WHERE s.owner_user_id=? ORDER BY s.updated_at DESC,s.id DESC LIMIT 50",
    )
        .bind(user_id)
        .fetch_all(pool)
        .await
        .map_err(AppError::Database)?;

    rows.into_iter()
        .map(|row| {
            let schema_version =
                crate::skill_schema::parse_skill_markdown(&row.skill_markdown)?.schema_version;
            Ok(UserSkillSummaryResponse {
                id: row.id,
                name: row.name,
                description: row.description,
                schema_version,
                content_hash: row.content_hash,
                version: row.version,
                enabled: row.enabled,
                created_at: row.created_at,
                updated_at: row.updated_at,
                review: parse_review_row(
                    row.review_overall_score,
                    row.review_grade,
                    row.review_dimensions,
                    row.review_findings,
                    row.review_evaluated_at,
                ),
            })
        })
        .collect()
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
    let inserted = sqlx::query("INSERT INTO user_skills(id,owner_user_id,name,description,skill_markdown,content_hash,enabled) SELECT ?,?,?,?,?,?,? WHERE (SELECT COUNT(*) FROM user_skills WHERE owner_user_id=?) < ?")
        .bind(&id)
        .bind(user_id)
        .bind(payload.name.trim())
        .bind(payload.description.as_deref().map(str::trim).filter(|value| !value.is_empty()))
        .bind(&payload.skill_markdown)
        .bind(content_hash)
        .bind(payload.enabled)
        .bind(user_id)
        .bind(MAX_SKILLS_PER_USER)
        .execute(pool)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
    if inserted != 1 {
        return Err(AppError::api(
            actix_web::http::StatusCode::CONFLICT,
            "SKILL_LIMIT_REACHED",
            "每个用户最多可以创建 50 个 Skill",
        ));
    }
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
    let schema_version =
        crate::skill_schema::parse_skill_markdown(&record.skill_markdown)?.schema_version;
    let row: Option<(i64, String, String, String, String)> = sqlx::query_as("SELECT overall_score,grade,dimension_scores_json,findings_json,evaluated_at FROM skill_reviews WHERE skill_id=? AND skill_version=? AND skill_content_hash=?")
        .bind(&record.id).bind(record.version).bind(&record.content_hash)
        .fetch_optional(pool).await.map_err(AppError::Database)?;
    let review = row.and_then(|(score, grade, dimensions, findings, evaluated_at)| {
        parse_review_row(
            Some(score),
            Some(grade),
            Some(dimensions),
            Some(findings),
            Some(evaluated_at),
        )
    });
    Ok(UserSkillResponse {
        id: record.id,
        name: record.name,
        description: record.description,
        skill_markdown: record.skill_markdown,
        schema_version,
        content_hash: record.content_hash,
        version: record.version,
        enabled: record.enabled,
        created_at: record.created_at,
        updated_at: record.updated_at,
        review,
    })
}

fn parse_review_row(
    overall_score: Option<i64>,
    grade: Option<String>,
    dimensions: Option<String>,
    findings: Option<String>,
    evaluated_at: Option<String>,
) -> Option<SkillReview> {
    let findings: serde_json::Value = serde_json::from_str(&findings?).ok()?;
    Some(SkillReview {
        overall_score: overall_score?,
        grade: grade?,
        dimensions: serde_json::from_str(&dimensions?).ok()?,
        warnings: serde_json::from_value(findings.get("warnings")?.clone()).ok()?,
        suggestions: serde_json::from_value(findings.get("suggestions")?.clone()).ok()?,
        evaluated_at,
    })
}
