use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::error::AppError;

pub fn init_pool(database_url: &str) -> Result<PgPool, AppError> {
    PgPoolOptions::new()
        .max_connections(5)
        .connect_lazy(database_url)
        .map_err(AppError::Database)
}
