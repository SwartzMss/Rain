use std::{env, path::PathBuf};

use crate::error::AppError;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub data_root: PathBuf,
    pub log_dir: PathBuf,
    pub reset_db: bool,
    pub retention_days: Option<u64>,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AppError> {
        dotenvy::dotenv().ok();

        let host = env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".into());
        let port: u16 = env::var("SERVER_PORT")
            .unwrap_or_else(|_| "8078".into())
            .parse()
            .map_err(|err| AppError::Config(format!("invalid SERVER_PORT: {err}")))?;

        let database_url =
            env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://./data/rain.db".into());

        let data_root =
            PathBuf::from(env::var("RAIN_DATA_ROOT").unwrap_or_else(|_| "./data/uploads".into()));

        let log_dir = PathBuf::from(env::var("RAIN_LOG_DIR").unwrap_or_else(|_| "./log".into()));

        let reset_db = env::var("RESET_DB")
            .unwrap_or_else(|_| "false".into())
            .parse::<bool>()
            .map_err(|err| AppError::Config(format!("invalid RESET_DB: {err}")))?;

        let retention_days = match env::var("RAIN_RETENTION_DAYS") {
            Ok(value) if !value.trim().is_empty() => {
                let days = value.parse::<u64>().map_err(|err| {
                    AppError::Config(format!("invalid RAIN_RETENTION_DAYS: {err}"))
                })?;
                if days == 0 { None } else { Some(days) }
            }
            _ => None,
        };

        Ok(Self {
            host,
            port,
            database_url,
            data_root,
            log_dir,
            reset_db,
            retention_days,
        })
    }
}
