mod embedded_frontend;
mod http_access_log;

use std::{fmt::Display, fs, future::Future, path::PathBuf, time::Duration};

use actix_web::{App, HttpServer, middleware::from_fn, web};
use backend::{
    AppState,
    blob_store::{
        BlobStore, LocalCasBlobStore, recover_pending_blobs, spawn_blob_audit, spawn_blob_gc,
        spawn_blob_recovery,
    },
    config::AppConfig,
    db::{
        cleanup_expired_bundles, fail_stale_processing_bundles, init_pool,
        load_or_initialize_system_settings, prepare_schema, resume_deleting_bundles,
    },
    routes::register,
};
use tracing::{error, info, warn};
use tracing_appender::rolling;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

const STARTUP_RECOVERY_TIMEOUT: Duration = Duration::from_secs(15);

struct SqliteSidecarPaths {
    main: PathBuf,
    wal: PathBuf,
    shm: PathBuf,
}

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
    backend::repositories::bootstrap_admin::bootstrap_admin(
        &pool,
        &config.bootstrap_admin.username,
        config.bootstrap_admin.password(),
    )
    .await
    .expect("failed to bootstrap administrator");
    let admin_username_normalized: String = sqlx::query_scalar(
        "SELECT username_normalized FROM users WHERE role='ADMIN' AND status='ACTIVE'",
    )
    .fetch_one(&pool)
    .await
    .expect("failed to load administrator identity");
    let (registration_value, ip_limit, username_limit, issue_inactive_days) =
        load_or_initialize_system_settings(
            &pool,
            config.auth.allow_registration,
            config.auth.login_ip_limit_per_minute,
            config.auth.login_username_failure_limit_per_5_minutes,
            config.issue_inactive_days,
        )
        .await
        .expect("failed to initialize auth rate limits");
    let registration_allowed = registration_value != 0;
    log_sqlite_file_sizes(&config.database_url).await;

    if config.reset_db {
        if fs::metadata(&config.data_root).is_ok() {
            let _ = fs::remove_dir_all(&config.data_root);
        }
        fs::create_dir_all(&config.data_root).expect("failed to recreate data root");
    }

    let blob_store: std::sync::Arc<dyn BlobStore> =
        std::sync::Arc::new(LocalCasBlobStore::new(config.data_root.clone()));
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

    run_optional_recovery_stage(
        "deleting-bundle-recovery",
        STARTUP_RECOVERY_TIMEOUT,
        resume_deleting_bundles(&pool),
    )
    .await;
    run_optional_recovery_stage("pending-blob-recovery", STARTUP_RECOVERY_TIMEOUT, async {
        recover_pending_blobs(&pool, blob_store.as_ref())
            .await
            .map(|stats| stats.recovered)
    })
    .await;

    if let Some(retention_days) = config.retention_days {
        run_optional_recovery_stage(
            "expired-bundle-cleanup",
            STARTUP_RECOVERY_TIMEOUT,
            cleanup_expired_bundles(&pool, retention_days),
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
    let mut background_tasks = vec![
        spawn_blob_gc(pool.clone(), blob_store.clone()),
        spawn_blob_audit(pool.clone(), blob_store.clone()),
        spawn_blob_recovery(pool.clone(), blob_store.clone()),
    ];
    background_tasks.push(spawn_deleting_bundle_cleanup(pool.clone()));
    background_tasks.push(spawn_session_cleanup(pool.clone()));
    let app_state = AppState::with_blob_store_and_auth(
        pool,
        config.data_root.clone(),
        config.limits.clone(),
        config.auth.clone(),
        blob_store,
    );
    app_state
        .auth_runtime
        .set_registration_allowed(registration_allowed);
    app_state
        .auth_runtime
        .admin_username_normalized
        .set(admin_username_normalized)
        .expect("administrator identity initialized once");
    app_state
        .auth_runtime
        .login_ip_limit_per_minute
        .store(ip_limit, std::sync::atomic::Ordering::Release);
    app_state
        .auth_runtime
        .login_username_failure_limit_per_5_minutes
        .store(username_limit, std::sync::atomic::Ordering::Release);
    app_state
        .issue_inactive_days
        .store(issue_inactive_days, std::sync::atomic::Ordering::Release);
    let shared_state = web::Data::new(app_state);
    background_tasks.push(backend::upload::job::spawn_temp_cleanup_worker(
        shared_state.upload.temp_cleanup_queue.clone(),
    ));
    background_tasks.push(backend::routes::spawn_temp_result_cleanup(
        shared_state.clone(),
    ));
    background_tasks.push(backend::routes::spawn_inactive_issue_cleanup(
        shared_state.clone(),
    ));
    background_tasks.push(backend::routes::spawn_manual_issue_cleanup(
        shared_state.clone(),
    ));

    let server = HttpServer::new(move || {
        App::new()
            .wrap(from_fn(http_access_log::log_useful_requests))
            .wrap(from_fn(backend::auth::same_origin::enforce_same_origin))
            .app_data(shared_state.clone())
            .configure(register)
            .default_service(web::get().to(embedded_frontend::serve_frontend))
    })
    .bind(bind_addr)?
    .run();
    let result = server.await;
    for task in background_tasks {
        task.abort();
    }
    result
}

fn spawn_deleting_bundle_cleanup(pool: sqlx::SqlitePool) -> tokio::task::JoinHandle<()> {
    backend::spawn_periodic_job(
        "deleting-bundle-cleanup",
        Duration::from_secs(30),
        Duration::from_secs(300),
        move || {
            let pool = pool.clone();
            async move {
                resume_deleting_bundles(&pool)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }
        },
    )
}

fn spawn_session_cleanup(pool: sqlx::SqlitePool) -> tokio::task::JoinHandle<()> {
    backend::spawn_periodic_job(
        "session-cleanup",
        Duration::ZERO,
        Duration::from_secs(60 * 60),
        move || {
            let pool = pool.clone();
            async move {
                backend::repositories::sessions::cleanup_expired_or_revoked(&pool)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }
        },
    )
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

fn sqlite_sidecar_paths(database_url: &str) -> Option<SqliteSidecarPaths> {
    let main = database_url
        .strip_prefix("sqlite://")
        .map(PathBuf::from)
        .map(|path| absolute_diagnostic_path(&path))?;
    let wal = PathBuf::from(format!("{}-wal", main.display()));
    let shm = PathBuf::from(format!("{}-shm", main.display()));
    Some(SqliteSidecarPaths { main, wal, shm })
}

async fn log_sqlite_file_sizes(database_url: &str) {
    let Some(paths) = sqlite_sidecar_paths(database_url) else {
        return;
    };
    for (kind, path) in [("main", paths.main), ("wal", paths.wal), ("shm", paths.shm)] {
        match tokio::fs::metadata(&path).await {
            Ok(metadata) => info!(
                database_file = kind,
                path = %path.display(),
                size_bytes = metadata.len(),
                "SQLite database file size"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => warn!(
                database_file = kind,
                path = %path.display(),
                error = %error,
                "failed to inspect SQLite database file size"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{run_optional_recovery_stage, sqlite_sidecar_paths};

    #[test]
    fn resolves_sqlite_main_wal_and_shm_paths() {
        let paths = sqlite_sidecar_paths("sqlite://data/rain.db").expect("sqlite paths");
        assert!(paths.main.ends_with("data/rain.db"));
        assert!(paths.wal.ends_with("data/rain.db-wal"));
        assert!(paths.shm.ends_with("data/rain.db-shm"));
        assert!(sqlite_sidecar_paths("postgres://localhost/rain").is_none());
    }

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
