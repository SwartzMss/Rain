use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    error::AppError,
    models::skill_runs::{NewSkillRun, SkillRunRecord},
};

const COLUMNS: &str = "id,user_id,issue_code,skill_id,skill_version,skill_name,skill_snapshot_markdown,status,iteration_count,tool_call_count,cancel_requested,result_json,error_code,error_message,started_at,completed_at,created_at";

pub async fn create(pool: &SqlitePool, value: &NewSkillRun) -> Result<SkillRunRecord, AppError> {
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO skill_runs(id,user_id,issue_code,skill_id,skill_version,skill_name,skill_snapshot_markdown,status) VALUES(?,?,?,?,?,?,?,'QUEUED')")
        .bind(&id).bind(&value.user_id).bind(&value.issue_code).bind(&value.skill_id)
        .bind(value.skill_version).bind(&value.skill_name).bind(&value.skill_snapshot_markdown)
        .execute(pool).await.map_err(AppError::Database)?;
    find(pool, &id)
        .await?
        .ok_or_else(|| AppError::Config("created Skill run is missing".into()))
}

pub async fn find(pool: &SqlitePool, id: &str) -> Result<Option<SkillRunRecord>, AppError> {
    let sql = format!("SELECT {COLUMNS} FROM skill_runs WHERE id=?");
    sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(AppError::Database)
}

pub async fn find_owned(
    pool: &SqlitePool,
    id: &str,
    user_id: &str,
) -> Result<Option<SkillRunRecord>, AppError> {
    let sql = format!("SELECT {COLUMNS} FROM skill_runs WHERE id=? AND user_id=?");
    sqlx::query_as(&sql)
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(AppError::Database)
}

pub async fn mark_running(pool: &SqlitePool, id: &str) -> Result<bool, AppError> {
    Ok(sqlx::query("UPDATE skill_runs SET status='RUNNING',started_at=CURRENT_TIMESTAMP WHERE id=? AND status='QUEUED' AND cancel_requested=0")
        .bind(id).execute(pool).await.map_err(AppError::Database)?.rows_affected() == 1)
}

pub async fn update_progress(
    pool: &SqlitePool,
    id: &str,
    iterations: usize,
    calls: usize,
) -> Result<bool, AppError> {
    Ok(sqlx::query("UPDATE skill_runs SET iteration_count=?,tool_call_count=? WHERE id=? AND status='RUNNING' AND cancel_requested=0")
        .bind(iterations as i64).bind(calls as i64).bind(id)
        .execute(pool).await.map_err(AppError::Database)?.rows_affected() == 1)
}

pub async fn cancel(pool: &SqlitePool, id: &str, user_id: &str) -> Result<bool, AppError> {
    Ok(sqlx::query("UPDATE skill_runs SET cancel_requested=1,status='CANCELLED',completed_at=CURRENT_TIMESTAMP,error_code=NULL,error_message=NULL WHERE id=? AND user_id=? AND status IN ('QUEUED','RUNNING')")
        .bind(id).bind(user_id).execute(pool).await.map_err(AppError::Database)?.rows_affected() == 1)
}

pub async fn complete(pool: &SqlitePool, id: &str, result_json: &str) -> Result<bool, AppError> {
    Ok(sqlx::query("UPDATE skill_runs SET status='SUCCEEDED',result_json=?,completed_at=CURRENT_TIMESTAMP WHERE id=? AND status='RUNNING' AND cancel_requested=0")
        .bind(result_json).bind(id).execute(pool).await.map_err(AppError::Database)?.rows_affected() == 1)
}

pub async fn fail(
    pool: &SqlitePool,
    id: &str,
    code: &str,
    message: &str,
) -> Result<bool, AppError> {
    Ok(sqlx::query("UPDATE skill_runs SET status='FAILED',error_code=?,error_message=?,completed_at=CURRENT_TIMESTAMP WHERE id=? AND status IN ('QUEUED','RUNNING') AND cancel_requested=0")
        .bind(code).bind(message).bind(id).execute(pool).await.map_err(AppError::Database)?.rows_affected() == 1)
}

pub async fn recover_active(pool: &SqlitePool) -> Result<u64, AppError> {
    Ok(sqlx::query("UPDATE skill_runs SET status='FAILED',error_code='SERVICE_RESTARTED',error_message='服务重启导致任务中断',completed_at=CURRENT_TIMESTAMP WHERE status IN ('QUEUED','RUNNING')")
        .execute(pool).await.map_err(AppError::Database)?.rows_affected())
}

pub async fn cleanup_expired(pool: &SqlitePool, retention_seconds: u64) -> Result<u64, AppError> {
    let modifier = format!("-{retention_seconds} seconds");
    Ok(sqlx::query("DELETE FROM skill_runs WHERE status IN ('SUCCEEDED','FAILED','CANCELLED') AND datetime(completed_at) <= datetime('now', ?)")
        .bind(modifier).execute(pool).await.map_err(AppError::Database)?.rows_affected())
}
