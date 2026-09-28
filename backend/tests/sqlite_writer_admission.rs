use std::{path::PathBuf, sync::Arc, time::Duration};

use actix_web::{App, cookie::Cookie, test, web};
use backend::{
    AppState,
    auth::session::{SESSION_COOKIE_NAME, generate_session_token, hash_session_token},
    config::AppLimits,
    db,
    repositories::{sessions, users},
    routes,
};
use chrono::{Duration as ChronoDuration, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use tokio::sync::Notify;

async fn file_pools() -> (PathBuf, sqlx::SqlitePool, sqlx::SqlitePool) {
    let root = std::env::temp_dir().join(format!("rain-writer-admission-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let options = SqliteConnectOptions::new()
        .filename(root.join("rain.db"))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_millis(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .unwrap();
    let external = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    (root, pool, external)
}

#[tokio::test]
async fn runtime_user_and_issue_writes_use_admission() {
    let (root, pool, external) = file_pools().await;

    let user = match users::create_user(&pool, "writer-owner", "hash")
        .await
        .unwrap()
    {
        users::CreateUserOutcome::Created(user) => user,
        users::CreateUserOutcome::DuplicateUsername => panic!("fixture username is unique"),
    };

    let first_token = generate_session_token();
    sessions::create_session(
        &pool,
        &user.id,
        &hash_session_token(&first_token),
        Utc::now() + ChronoDuration::hours(1),
        None,
        None,
    )
    .await
    .unwrap();

    let mut blocker = external.begin().await.unwrap();
    sqlx::query("UPDATE users SET status = 'ACTIVE' WHERE id = ?")
        .bind(&user.id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let queued = Arc::new(Notify::new());
    let closure_started = Arc::new(Notify::new());
    let probe_pool = pool.clone();
    let probe_user_id = user.id.clone();
    let queued_signal = queued.clone();
    let closure_started_signal = closure_started.clone();
    let probe = tokio::spawn(async move {
        queued_signal.notify_one();
        db::write::run(
            &probe_pool,
            "writer admission integration probe",
            &probe_user_id,
            |conn, user_id| {
                let closure_started_signal = closure_started_signal.clone();
                Box::pin(async move {
                    closure_started_signal.notify_one();
                    sqlx::query("UPDATE users SET updated_at=CURRENT_TIMESTAMP WHERE id=?")
                        .bind(user_id)
                        .execute(conn)
                        .await
                        .map(|_| ())
                        .map_err(backend::error::AppError::Database)
                })
            },
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(1), queued.notified())
        .await
        .expect("writer probe should be scheduled");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), closure_started.notified())
            .await
            .is_err(),
        "writer closure must not run before BEGIN IMMEDIATE is admitted"
    );
    blocker.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), closure_started.notified())
        .await
        .expect("writer closure should run after the external writer releases");
    probe.await.unwrap().unwrap();

    let mut blocker = external.begin().await.unwrap();
    sqlx::query("UPDATE users SET status = 'ACTIVE' WHERE id = ?")
        .bind(&user.id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(75)).await;
        blocker.rollback().await.unwrap();
    });
    let created = tokio::time::timeout(
        Duration::from_secs(2),
        users::create_user(&pool, "writer-second", "hash"),
    )
    .await
    .expect("user write should wait for and outlive the external writer")
    .unwrap();
    release.await.unwrap();
    assert!(matches!(created, users::CreateUserOutcome::Created(_)));

    let state = web::Data::new(AppState::new(
        pool.clone(),
        root.clone(),
        AppLimits::default(),
    ));
    let app = test::init_service(
        App::new()
            .app_data(state.clone())
            .configure(routes::register),
    )
    .await;

    let mut blocker = external.begin().await.unwrap();
    sqlx::query("UPDATE users SET status = 'ACTIVE' WHERE id = ?")
        .bind(&user.id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(75)).await;
        blocker.rollback().await.unwrap();
    });
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/issues")
                .cookie(Cookie::new(SESSION_COOKIE_NAME, first_token))
                .set_json(serde_json::json!({"code": "ADMITTED", "name": "Admitted"}))
                .to_request(),
        ),
    )
    .await
    .expect("issue write should wait for and outlive the external writer");
    release.await.unwrap();
    assert_eq!(response.status(), actix_web::http::StatusCode::CREATED);

    drop(app);
    drop(state);
    pool.close().await;
    external.close().await;
    tokio::fs::remove_dir_all(root).await.unwrap();
}
