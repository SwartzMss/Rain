mod embedded_frontend;

use std::{fmt::Display, fs, future::Future, path::PathBuf, time::Duration};

use actix_cors::Cors;
use actix_web::{App, HttpServer, middleware::Logger, web};
use backend::{
    AppState,
    config::AppConfig,
    db::{cleanup_expired_bundles, fail_stale_processing_bundles, init_pool, prepare_schema},
    routes::register,
};
use tracing::{error, info, warn};
use tracing_appender::rolling;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

const STARTUP_RECOVERY_TIMEOUT: Duration = Duration::from_secs(15);

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let config = AppConfig::from_env().expect("failed to load config");

    fs::create_dir_all(&config.log_dir).expect("failed to create log directory");
    let file_appender = rolling::daily(&config.log_dir, "backend.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    let _guard = guard;

    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .expect("failed to init logging filter");

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer())
        .with(fmt::layer().with_ansi(false).with_writer(file_writer))
        .init();

    info!(
        database_url = %config.database_url,
        database_path = %sqlite_diagnostic_path(&config.database_url).display(),
        data_root = %absolute_diagnostic_path(&config.data_root).display(),
        log_dir = %absolute_diagnostic_path(&config.log_dir).display(),
        "resolved startup paths"
    );

    let pool = init_pool(&config.database_url).expect("failed to init sqlite pool");
    prepare_schema(&pool, config.reset_db)
        .await
        .expect("failed to prepare database schema");

    if config.reset_db {
        if fs::metadata(&config.data_root).is_ok() {
            let _ = fs::remove_dir_all(&config.data_root);
        }
        fs::create_dir_all(&config.data_root).expect("failed to recreate data root");
    }

    run_optional_recovery_stage(
        "stale-processing-bundles",
        STARTUP_RECOVERY_TIMEOUT,
        fail_stale_processing_bundles(&pool),
    )
    .await;

    run_optional_recovery_stage(
        "temporary-upload-cleanup",
        STARTUP_RECOVERY_TIMEOUT,
        cleanup_temp_uploads(&config.data_root),
    )
    .await;

    if let Some(retention_days) = config.retention_days {
        run_optional_recovery_stage(
            "expired-bundle-cleanup",
            STARTUP_RECOVERY_TIMEOUT,
            cleanup_expired_bundles(&pool, &config.data_root, retention_days),
        )
        .await;
    }

    info!(
        host = %config.host,
        port = config.port,
        "starting Rain backend"
    );

    let bind_addr = format!("{}:{}", config.host, config.port);
    info!(limits = ?config.limits, "effective application limits");
    let shared_state = web::Data::new(AppState::new(
        pool,
        config.data_root.clone(),
        config.limits.clone(),
    ));

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .wrap(Cors::permissive())
            .app_data(shared_state.clone())
            .configure(register)
            .default_service(web::get().to(embedded_frontend::serve_frontend))
    })
    .bind(bind_addr)?
    .run()
    .await
}

async fn cleanup_temp_uploads(data_root: &std::path::Path) -> std::io::Result<u64> {
    let temp_root = data_root.join(".tmp");
    match tokio::fs::metadata(&temp_root).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    }

    let mut removed = 0u64;
    let mut failed = 0u64;
    let mut entries = tokio::fs::read_dir(&temp_root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let result = match entry.file_type().await {
            Ok(file_type) if file_type.is_dir() => tokio::fs::remove_dir_all(&path).await,
            Ok(_) => tokio::fs::remove_file(&path).await,
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            failed += 1;
            warn!(path = %path.display(), error = %error, "failed to remove stale temporary upload entry");
        } else {
            removed += 1;
        }
    }

    info!(removed, failed, "temporary upload cleanup summary");

    Ok(removed)
}

async fn run_optional_recovery_stage<F, E>(
    stage: &'static str,
    timeout: Duration,
    future: F,
) -> bool
where
    F: Future<Output = Result<u64, E>>,
    E: Display,
{
    let started = std::time::Instant::now();
    info!(
        stage,
        timeout_ms = timeout.as_millis(),
        "startup recovery stage started"
    );
    match tokio::time::timeout(timeout, future).await {
        Ok(Ok(affected)) => {
            info!(
                stage,
                affected,
                elapsed_ms = started.elapsed().as_millis(),
                "startup recovery stage completed"
            );
            true
        }
        Ok(Err(stage_error)) => {
            error!(
                stage,
                error = %stage_error,
                elapsed_ms = started.elapsed().as_millis(),
                "startup recovery stage failed; continuing startup"
            );
            false
        }
        Err(_) => {
            error!(
                stage,
                timeout_ms = timeout.as_millis(),
                elapsed_ms = started.elapsed().as_millis(),
                "startup recovery stage timed out; continuing startup"
            );
            false
        }
    }
}

fn absolute_diagnostic_path(path: &std::path::Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn sqlite_diagnostic_path(database_url: &str) -> PathBuf {
    database_url
        .strip_prefix("sqlite://")
        .map(PathBuf::from)
        .map(|path| absolute_diagnostic_path(&path))
        .unwrap_or_else(|| PathBuf::from("<non-sqlite-database>"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::run_optional_recovery_stage;

    #[actix_web::test]
    async fn optional_recovery_error_does_not_abort_startup() {
        let completed =
            run_optional_recovery_stage("test-error", Duration::from_millis(20), async {
                Err::<u64, _>("expected failure")
            })
            .await;
        assert!(!completed);
    }

    #[actix_web::test]
    async fn optional_recovery_timeout_returns_control() {
        let completed =
            run_optional_recovery_stage("test-timeout", Duration::from_millis(5), async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok::<u64, &str>(0)
            })
            .await;
        assert!(!completed);
    }
}
