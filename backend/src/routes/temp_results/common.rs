use super::AppError;

pub(crate) fn checked_page_end(start: i64, limit: i64) -> Result<i64, AppError> {
    start
        .checked_add(limit)
        .ok_or_else(|| AppError::BadRequest("分页参数超出支持范围".into()))
}
