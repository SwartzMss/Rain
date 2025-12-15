mod config;
mod db;
mod error;
mod ingest;
mod models;
mod routes;

use std::{fs, path::PathBuf};

use actix_cors::Cors;
use actix_web::{App, HttpServer, middleware::Logger, web};
use config::AppConfig;
use db::{init_pool, prepare_schema};
use routes::register;
use sqlx::PgPool;
use tracing::info;
use tracing_appender::rolling;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub struct AppState {
    pub pool: PgPool,
    pub data_root: PathBuf,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();
    let config = AppConfig::from_env().expect("failed to load config");

    fs::create_dir_all(&config.log_dir).expect("failed to create log directory");
    let file_appender = rolling::never(&config.log_dir, "backend.log");
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

    let pool = init_pool(&config.database_url).expect("failed to init postgres pool");
    prepare_schema(&pool, config.reset_db)
        .await
        .expect("failed to prepare database schema");

    info!(
        host = %config.host,
        port = config.port,
        "starting Rain backend"
    );

    let bind_addr = format!("{}:{}", config.host, config.port);
    let shared_state = web::Data::new(AppState {
        pool,
        data_root: config.data_root.clone(),
    });

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .wrap(Cors::permissive())
            .app_data(shared_state.clone())
            .configure(register)
    })
    .bind(bind_addr)?
    .run()
    .await
}
