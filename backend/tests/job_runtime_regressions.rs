use actix_web::{App, HttpResponse, HttpServer, web};
use backend::{JobRuntime, JobType};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[actix_web::test]
async fn http_spawned_job_should_receive_shutdown_grace() {
    let jobs = JobRuntime::new();
    let completed = Arc::new(AtomicBool::new(false));
    let server_jobs = jobs.clone();
    let server_completed = completed.clone();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = HttpServer::new(move || {
        let jobs = server_jobs.clone();
        let completed = server_completed.clone();
        App::new().route(
            "/",
            web::get().to(move || {
                let jobs = jobs.clone();
                let completed = completed.clone();
                async move {
                    jobs.spawn(JobType::Upload, move |_context| async move {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        completed.store(true, Ordering::SeqCst);
                        Ok::<(), String>(())
                    })
                    .unwrap();
                    HttpResponse::Accepted().finish()
                }
            }),
        )
    })
    .workers(1)
    .disable_signals()
    .listen(listener)
    .unwrap()
    .run();
    let handle = server.handle();
    let server_task = actix_web::rt::spawn(server);
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    assert!(String::from_utf8_lossy(&response).contains("202"));
    handle.stop(true).await;
    let (_, report) = tokio::join!(server_task, jobs.shutdown(Duration::from_secs(3)));
    eprintln!(
        "report={report:?}, metrics={:?}",
        jobs.snapshot().for_type(JobType::Upload)
    );
    assert!(
        completed.load(Ordering::SeqCst),
        "accepted upload was aborted before the job grace period"
    );
}

#[tokio::test]
async fn periodic_failure_should_be_counted() {
    let jobs = JobRuntime::new();
    let ran = Arc::new(tokio::sync::Notify::new());
    let worker_ran = ran.clone();
    jobs.spawn_periodic_job(
        JobType::Index,
        "review-probe",
        Duration::ZERO,
        Duration::from_secs(3600),
        move || {
            let ran = worker_ran.clone();
            async move {
                ran.notify_one();
                Err("rebuild failed".to_owned())
            }
        },
    )
    .unwrap();
    ran.notified().await;
    tokio::task::yield_now().await;
    let metrics = jobs.snapshot().for_type(JobType::Index);
    jobs.shutdown(Duration::from_millis(100)).await;
    assert_eq!(
        metrics.failed_total, 1,
        "completed periodic failure is invisible: {metrics:?}"
    );
}

async fn finalizing_session(file_name: &str) -> (backend::AppState, std::path::PathBuf) {
    use backend::{
        config::AppLimits,
        db,
        repositories::users::{self, CreateUserOutcome},
        upload::session::{CHUNK_SIZE_BYTES, CreateSessionRow, create_session},
    };
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let user = match users::create_user(&pool, "review-owner", "hash")
        .await
        .unwrap()
    {
        CreateUserOutcome::Created(user) => user,
        _ => panic!("duplicate user"),
    };
    sqlx::query("INSERT INTO issues (code, name, owner_user_id) VALUES ('REVIEW', 'Review', ?)")
        .bind(&user.id)
        .execute(&pool)
        .await
        .unwrap();
    let root = std::env::temp_dir().join(format!("rain-review-277-{}", uuid::Uuid::new_v4()));
    let input = root.join(".uploads/review-session/input.part");
    tokio::fs::create_dir_all(input.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&input, b"not a valid zip").await.unwrap();
    create_session(
        &pool,
        CreateSessionRow {
            id: "review-session".into(),
            issue_code: "REVIEW".into(),
            owner_user_id: user.id,
            idempotency_key: "review".into(),
            file_name: file_name.into(),
            file_size_bytes: 15,
            last_modified_ms: None,
            chunk_size_bytes: CHUNK_SIZE_BYTES,
            input_path: ".uploads/review-session/input.part".into(),
            expires_at: "2099-01-01 00:00:00".into(),
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE upload_sessions SET status='FINALIZING', committed_offset=15 WHERE id='review-session'").execute(&pool).await.unwrap();
    let state = backend::AppState::new(pool, root.clone(), AppLimits::default());
    state.upload.tmp_bytes.store(15, Ordering::SeqCst);
    (state, root)
}

#[tokio::test]
async fn rejected_handoff_should_not_mark_session_delivered() {
    let (state, root) = finalizing_session("review.log").await;
    state.jobs.shutdown(Duration::ZERO).await;
    let result = backend::upload::session_finalizer::finalize_one(&state, "review-session").await;
    let status: String =
        sqlx::query_scalar("SELECT status FROM upload_sessions WHERE id='review-session'")
            .fetch_one(&state.db.pool)
            .await
            .unwrap();
    tokio::fs::remove_dir_all(&root).await.unwrap();
    assert!(
        result.is_err() || status != "DELIVERED",
        "rejected upload was marked delivered; status={status}"
    );
}

#[tokio::test]
async fn corrupt_upload_should_increment_failed_total() {
    let (state, root) = finalizing_session("review.zip").await;
    backend::upload::session_finalizer::finalize_one(&state, "review-session")
        .await
        .unwrap();
    let metrics = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let metrics = state.jobs.snapshot().for_type(JobType::Upload);
            if metrics.completed_total + metrics.failed_total > 0 {
                break metrics;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM bundles WHERE issue_code='REVIEW'")
        .fetch_one(&state.db.pool)
        .await
        .unwrap();
    state.jobs.shutdown(Duration::ZERO).await;
    tokio::fs::remove_dir_all(&root).await.unwrap();
    assert_eq!(status, "FAILED");
    assert_eq!(
        metrics.failed_total, 1,
        "failed bundle counted as successful: {metrics:?}"
    );
}
