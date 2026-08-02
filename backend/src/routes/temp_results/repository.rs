use super::*;

pub(crate) async fn delete_temp_result_record(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM temp_results WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(AppError::Database)?;
    Ok(())
}
