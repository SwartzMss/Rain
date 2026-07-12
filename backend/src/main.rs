use std::{fs, path::PathBuf};

use actix_cors::Cors;
use actix_files::{Files, NamedFile};
use actix_web::{App, HttpServer, middleware::Logger, web};
use backend::{
    AppState,
    config::AppConfig,
    db::{cleanup_expired_bundles, fail_stale_processing_bundles, init_pool, prepare_schema},
    routes::register,
};
use tracing::info;
use tracing_appender::rolling;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone)]
struct StaticState {
    root: PathBuf,
}

async fn spa_index(static_state: web::Data<StaticState>) -> actix_web::Result<NamedFile> {
    Ok(NamedFile::open(static_state.root.join("index.html"))?)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();
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

    let stale = fail_stale_processing_bundles(&pool)
        .await
        .expect("failed to mark stale processing bundles failed");
    if stale > 0 {
        info!(stale, "marked stale processing bundles failed");
    }

    let removed_tmp = cleanup_temp_uploads(&config.data_root)
        .await
        .expect("failed to cleanup temp uploads");
    if removed_tmp > 0 {
        info!(removed_tmp, "cleaned up stale temp upload directories");
    }

    if let Some(retention_days) = config.retention_days {
        let removed = cleanup_expired_bundles(&pool, &config.data_root, retention_days)
            .await
            .expect("failed to cleanup expired bundles");
        if removed > 0 {
            info!(retention_days, removed, "cleaned up expired log bundles");
        }
    }

    info!(
        host = %config.host,
        port = config.port,
        static_root = %config.static_root.display(),
        "starting Rain backend"
    );

    let bind_addr = format!("{}:{}", config.host, config.port);
    let shared_state = web::Data::new(AppState {
        pool,
        data_root: config.data_root.clone(),
    });
    let static_root = config.static_root.clone();
    let serve_static = static_root.join("index.html").is_file();
    if serve_static {
        info!(
            static_root = %static_root.display(),
            "serving frontend static files"
        );
    } else {
        info!(
            static_root = %static_root.display(),
            "frontend dist not found; serving API only"
        );
    }

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .wrap(Cors::permissive())
            .app_data(shared_state.clone())
            .configure(register)
            .configure({
                let static_root = static_root.clone();
                move |cfg| {
                    if serve_static {
                        cfg.app_data(web::Data::new(StaticState {
                            root: static_root.clone(),
                        }))
                        .service(Files::new("/assets", static_root.join("assets")))
                        .default_service(web::get().to(spa_index));
                    }
                }
            })
    })
    .bind(bind_addr)?
    .run()
    .await
}

async fn cleanup_temp_uploads(data_root: &std::path::Path) -> std::io::Result<u64> {
    let temp_root = data_root.join(".tmp");
    if tokio::fs::metadata(&temp_root).await.is_err() {
        return Ok(0);
    }

    let mut removed = 0u64;
    let mut entries = tokio::fs::read_dir(&temp_root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if entry.file_type().await?.is_dir() {
            tokio::fs::remove_dir_all(&path).await?;
        } else {
            tokio::fs::remove_file(&path).await?;
        }
        removed += 1;
    }

    Ok(removed)
}
