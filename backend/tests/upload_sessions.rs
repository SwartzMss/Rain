use actix_web::{App, cookie::Cookie, test as actix_test, web};
use backend::{
    AppState,
    auth::session::{SESSION_COOKIE_NAME, generate_session_token, hash_session_token},
    config::AppLimits,
    db,
    repositories::{
        sessions,
        users::{self, CreateUserOutcome},
    },
    routes,
    upload::session::{
        CHUNK_SIZE_BYTES, CreateSessionRow, create_session, expected_chunk_size,
        find_by_idempotency,
    },
};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn expected_chunk_size_accepts_short_final_chunk_and_rejects_overflow() {
    assert_eq!(
        expected_chunk_size(17 * 1024 * 1024, 8 * 1024 * 1024, 0).unwrap(),
        8 * 1024 * 1024
    );
    assert_eq!(
        expected_chunk_size(17 * 1024 * 1024, 8 * 1024 * 1024, 2).unwrap(),
        1 * 1024 * 1024
    );
    assert!(expected_chunk_size(17 * 1024 * 1024, 8 * 1024 * 1024, 3).is_err());
}

#[tokio::test]
async fn session_schema_tracks_committed_offset_and_idempotency() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let user = match users::create_user(&pool, "session-owner", "hash")
        .await
        .unwrap()
    {
        CreateUserOutcome::Created(user) => user,
        CreateUserOutcome::DuplicateUsername => panic!("duplicate test user"),
    };
    sqlx::query("INSERT INTO issues (code, name, owner_user_id) VALUES ('SESSION', 'Session', ?)")
        .bind(&user.id)
        .execute(&pool)
        .await
        .unwrap();

    let input = CreateSessionRow {
        id: "session-1".into(),
        issue_code: "SESSION".into(),
        owner_user_id: user.id.clone(),
        idempotency_key: "key".into(),
        file_name: "large.log".into(),
        file_size_bytes: 64 * 1024 * 1024,
        last_modified_ms: Some(1),
        chunk_size_bytes: CHUNK_SIZE_BYTES,
        input_path: ".uploads/session-1/input.part".into(),
        expires_at: "2099-01-01 00:00:00".into(),
    };
    let session = create_session(&pool, input.clone()).await.unwrap();
    assert_eq!(session.committed_offset, 0);
    let same = find_by_idempotency(&pool, &user.id, "SESSION", "key")
        .await
        .unwrap();
    assert_eq!(same.id, session.id);

    let duplicate = create_session(
        &pool,
        CreateSessionRow {
            file_size_bytes: input.file_size_bytes + 1,
            id: "session-2".into(),
            ..input
        },
    )
    .await;
    assert!(matches!(
        duplicate,
        Err(backend::error::AppError::Conflict(_))
    ));
}

#[actix_web::test]
async fn create_session_is_idempotent_and_rejects_key_reuse_with_different_metadata() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let user = match users::create_user(&pool, "session-http-owner", "hash")
        .await
        .unwrap()
    {
        CreateUserOutcome::Created(user) => user,
        CreateUserOutcome::DuplicateUsername => panic!("duplicate test user"),
    };
    sqlx::query(
        "INSERT INTO issues (code, name, owner_user_id) VALUES ('SESSIONHTTP', 'Session HTTP', ?)",
    )
    .bind(&user.id)
    .execute(&pool)
    .await
    .unwrap();
    let token = generate_session_token();
    sessions::create_session(
        &pool,
        &user.id,
        &hash_session_token(&token),
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await
    .unwrap();
    let data_root = std::env::temp_dir().join(format!("rain-upload-session-{}", Uuid::new_v4()));
    let app = actix_test::init_service(
        App::new()
            .app_data(web::Data::new(AppState::new(
                pool.clone(),
                data_root.clone(),
                AppLimits::default(),
            )))
            .configure(routes::register),
    )
    .await;
    let cookie = Cookie::new(SESSION_COOKIE_NAME, token);
    let body = serde_json::json!({
        "file_name": "large.log",
        "file_size_bytes": 64 * 1024 * 1024,
        "last_modified_ms": 1,
        "idempotency_key": "browser-key"
    });

    let first = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/issues/SESSIONHTTP/upload-sessions")
            .cookie(cookie.clone())
            .set_json(&body)
            .to_request(),
    )
    .await;
    assert_eq!(first.status(), actix_web::http::StatusCode::CREATED);
    let first_body: serde_json::Value = actix_test::read_body_json(first).await;
    assert_eq!(first_body["committed_offset"], 0);
    assert_eq!(first_body["status"], "OPEN");
    let session_id = first_body["session_id"].as_str().unwrap().to_owned();

    let replay = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/issues/SESSIONHTTP/upload-sessions")
            .cookie(cookie.clone())
            .set_json(&body)
            .to_request(),
    )
    .await;
    assert_eq!(replay.status(), actix_web::http::StatusCode::OK);
    let replay_body: serde_json::Value = actix_test::read_body_json(replay).await;
    assert_eq!(replay_body["session_id"], session_id);

    let mut changed = body;
    changed["file_size_bytes"] = serde_json::json!(64 * 1024 * 1024 + 1);
    let conflict = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/issues/SESSIONHTTP/upload-sessions")
            .cookie(cookie)
            .set_json(changed)
            .to_request(),
    )
    .await;
    assert_eq!(conflict.status(), actix_web::http::StatusCode::CONFLICT);
    let _ = tokio::fs::remove_dir_all(data_root).await;
}

#[actix_web::test]
async fn session_lifecycle_is_owner_scoped_and_cancel_releases_capacity() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let owner = match users::create_user(&pool, "session-lifecycle-owner", "hash")
        .await
        .unwrap()
    {
        CreateUserOutcome::Created(user) => user,
        CreateUserOutcome::DuplicateUsername => panic!("duplicate test user"),
    };
    let foreign = match users::create_user(&pool, "session-lifecycle-foreign", "hash")
        .await
        .unwrap()
    {
        CreateUserOutcome::Created(user) => user,
        CreateUserOutcome::DuplicateUsername => panic!("duplicate test user"),
    };
    sqlx::query("INSERT INTO issues (code, name, owner_user_id) VALUES ('SESSIONOWNER', 'Session Owner', ?)")
        .bind(&owner.id)
        .execute(&pool)
        .await
        .unwrap();
    let owner_token = generate_session_token();
    sessions::create_session(
        &pool,
        &owner.id,
        &hash_session_token(&owner_token),
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await
    .unwrap();
    let foreign_token = generate_session_token();
    sessions::create_session(
        &pool,
        &foreign.id,
        &hash_session_token(&foreign_token),
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await
    .unwrap();
    let data_root = std::env::temp_dir().join(format!("rain-upload-session-{}", Uuid::new_v4()));
    let state = web::Data::new(AppState::new(
        pool.clone(),
        data_root.clone(),
        AppLimits::default(),
    ));
    let app = actix_test::init_service(
        App::new()
            .app_data(state.clone())
            .configure(routes::register),
    )
    .await;
    let owner_cookie = Cookie::new(SESSION_COOKIE_NAME, owner_token.clone());
    let foreign_cookie = Cookie::new(SESSION_COOKIE_NAME, foreign_token);
    let body = serde_json::json!({
        "file_name": "owned.log",
        "file_size_bytes": 64 * 1024 * 1024,
        "last_modified_ms": null,
        "idempotency_key": "owner-key"
    });
    let create = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/issues/SESSIONOWNER/upload-sessions")
            .cookie(owner_cookie.clone())
            .set_json(&body)
            .to_request(),
    )
    .await;
    assert_eq!(create.status(), actix_web::http::StatusCode::CREATED);
    let created: serde_json::Value = actix_test::read_body_json(create).await;
    let session_id = created["session_id"].as_str().unwrap().to_owned();
    assert_eq!(
        state
            .upload
            .tmp_bytes
            .load(std::sync::atomic::Ordering::Acquire),
        64 * 1024 * 1024
    );

    let foreign_get = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri(&format!("/api/upload-sessions/{session_id}"))
            .cookie(foreign_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(foreign_get.status(), actix_web::http::StatusCode::NOT_FOUND);

    let foreign_delete = actix_test::call_service(
        &app,
        actix_test::TestRequest::delete()
            .uri(&format!("/api/upload-sessions/{session_id}"))
            .cookie(foreign_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(
        foreign_delete.status(),
        actix_web::http::StatusCode::NOT_FOUND
    );

    let list = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/api/issues/SESSIONOWNER/upload-sessions")
            .cookie(owner_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(list.status(), actix_web::http::StatusCode::OK);
    let listed: serde_json::Value = actix_test::read_body_json(list).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let delete = actix_test::call_service(
        &app,
        actix_test::TestRequest::delete()
            .uri(&format!("/api/upload-sessions/{session_id}"))
            .cookie(owner_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(delete.status(), actix_web::http::StatusCode::OK);
    let deleted: serde_json::Value = actix_test::read_body_json(delete).await;
    assert_eq!(deleted["status"], "CANCELLED");
    assert_eq!(
        state
            .upload
            .tmp_bytes
            .load(std::sync::atomic::Ordering::Acquire),
        0
    );
    let listed_after: serde_json::Value = actix_test::read_body_json(
        actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/api/issues/SESSIONOWNER/upload-sessions")
                .cookie(owner_cookie)
                .to_request(),
        )
        .await,
    )
    .await;
    assert!(listed_after.as_array().unwrap().is_empty());
    let _ = tokio::fs::remove_dir_all(data_root).await;
}

#[actix_web::test]
async fn chunk_endpoint_requires_sequential_offsets_and_verifies_hashes() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let user = match users::create_user(&pool, "session-chunk-owner", "hash")
        .await
        .unwrap()
    {
        CreateUserOutcome::Created(user) => user,
        CreateUserOutcome::DuplicateUsername => panic!("duplicate test user"),
    };
    sqlx::query("INSERT INTO issues (code, name, owner_user_id) VALUES ('SESSIONCHUNK', 'Session Chunk', ?)")
        .bind(&user.id)
        .execute(&pool)
        .await
        .unwrap();
    let token = generate_session_token();
    sessions::create_session(
        &pool,
        &user.id,
        &hash_session_token(&token),
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await
    .unwrap();
    let data_root = std::env::temp_dir().join(format!("rain-upload-session-{}", Uuid::new_v4()));
    let session_id = "chunk-session";
    let session_dir = data_root.join(".uploads").join(session_id);
    tokio::fs::create_dir_all(&session_dir).await.unwrap();
    tokio::fs::File::create(session_dir.join("input.part"))
        .await
        .unwrap();
    create_session(
        &pool,
        CreateSessionRow {
            id: session_id.into(),
            issue_code: "SESSIONCHUNK".into(),
            owner_user_id: user.id.clone(),
            idempotency_key: "chunk-key".into(),
            file_name: "chunk.log".into(),
            file_size_bytes: 17 * 1024 * 1024,
            last_modified_ms: None,
            chunk_size_bytes: CHUNK_SIZE_BYTES,
            input_path: format!(".uploads/{session_id}/input.part"),
            expires_at: "2099-01-01 00:00:00".into(),
        },
    )
    .await
    .unwrap();
    let app = actix_test::init_service(
        App::new()
            .app_data(web::Data::new(AppState::new(
                pool.clone(),
                data_root.clone(),
                AppLimits::default(),
            )))
            .configure(routes::register),
    )
    .await;
    let cookie = Cookie::new(SESSION_COOKIE_NAME, token);
    let first_chunk = vec![b'a'; CHUNK_SIZE_BYTES as usize];
    let first_hash = format_digest(&first_chunk);
    let first = actix_test::call_service(
        &app,
        actix_test::TestRequest::put()
            .uri(&format!("/api/upload-sessions/{session_id}/chunks/0"))
            .cookie(cookie.clone())
            .insert_header(("X-Upload-Offset", "0"))
            .insert_header(("X-Chunk-SHA256", first_hash.as_str()))
            .insert_header(("Content-Length", first_chunk.len().to_string()))
            .set_payload(first_chunk.clone())
            .to_request(),
    )
    .await;
    assert_eq!(first.status(), actix_web::http::StatusCode::OK);
    let first_body: serde_json::Value = actix_test::read_body_json(first).await;
    assert_eq!(first_body["committed_offset"], CHUNK_SIZE_BYTES);
    assert_eq!(first_body["next_chunk_index"], 1);

    let duplicate = actix_test::call_service(
        &app,
        actix_test::TestRequest::put()
            .uri(&format!("/api/upload-sessions/{session_id}/chunks/0"))
            .cookie(cookie.clone())
            .insert_header(("X-Upload-Offset", "0"))
            .insert_header(("X-Chunk-SHA256", first_hash.as_str()))
            .insert_header(("Content-Length", first_chunk.len().to_string()))
            .set_payload(first_chunk.clone())
            .to_request(),
    )
    .await;
    assert_eq!(duplicate.status(), actix_web::http::StatusCode::OK);
    let duplicate_body: serde_json::Value = actix_test::read_body_json(duplicate).await;
    assert_eq!(duplicate_body["committed_offset"], CHUNK_SIZE_BYTES);

    let conflicting = actix_test::call_service(
        &app,
        actix_test::TestRequest::put()
            .uri(&format!("/api/upload-sessions/{session_id}/chunks/0"))
            .cookie(cookie.clone())
            .insert_header(("X-Upload-Offset", "0"))
            .insert_header((
                "X-Chunk-SHA256",
                format_digest(&vec![b'b'; CHUNK_SIZE_BYTES as usize]),
            ))
            .insert_header(("Content-Length", first_chunk.len().to_string()))
            .set_payload(first_chunk.clone())
            .to_request(),
    )
    .await;
    assert_eq!(conflicting.status(), actix_web::http::StatusCode::CONFLICT);

    let future = actix_test::call_service(
        &app,
        actix_test::TestRequest::put()
            .uri(&format!("/api/upload-sessions/{session_id}/chunks/2"))
            .cookie(cookie.clone())
            .insert_header(("X-Upload-Offset", CHUNK_SIZE_BYTES.to_string()))
            .insert_header(("X-Chunk-SHA256", first_hash.as_str()))
            .insert_header(("Content-Length", first_chunk.len().to_string()))
            .set_payload(first_chunk.clone())
            .to_request(),
    )
    .await;
    assert_eq!(future.status(), actix_web::http::StatusCode::CONFLICT);

    let bad_hash = actix_test::call_service(
        &app,
        actix_test::TestRequest::put()
            .uri(&format!("/api/upload-sessions/{session_id}/chunks/1"))
            .cookie(cookie.clone())
            .insert_header(("X-Upload-Offset", CHUNK_SIZE_BYTES.to_string()))
            .insert_header(("X-Chunk-SHA256", first_hash.as_str()))
            .insert_header(("Content-Length", first_chunk.len().to_string()))
            .set_payload(vec![b'b'; CHUNK_SIZE_BYTES as usize])
            .to_request(),
    )
    .await;
    assert_eq!(bad_hash.status(), actix_web::http::StatusCode::BAD_REQUEST);
    assert_eq!(
        tokio::fs::metadata(session_dir.join("input.part"))
            .await
            .unwrap()
            .len(),
        CHUNK_SIZE_BYTES
    );
    let _ = tokio::fs::remove_dir_all(data_root).await;
}

#[actix_web::test]
async fn complete_handoff_creates_one_bundle_and_startup_reconciles_tail_bytes() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let user = match users::create_user(&pool, "session-finalizer-owner", "hash")
        .await
        .unwrap()
    {
        CreateUserOutcome::Created(user) => user,
        CreateUserOutcome::DuplicateUsername => panic!("duplicate test user"),
    };
    sqlx::query("INSERT INTO issues (code, name, owner_user_id) VALUES ('SESSIONFINAL', 'Session Final', ?)")
        .bind(&user.id)
        .execute(&pool)
        .await
        .unwrap();
    let token = generate_session_token();
    sessions::create_session(
        &pool,
        &user.id,
        &hash_session_token(&token),
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await
    .unwrap();
    let data_root = std::env::temp_dir().join(format!("rain-upload-session-{}", Uuid::new_v4()));
    let session_id = "final-session";
    let file_size = 17 * 1024 * 1024;
    let session_dir = data_root.join(".uploads").join(session_id);
    tokio::fs::create_dir_all(&session_dir).await.unwrap();
    let mut content = Vec::with_capacity(file_size);
    while content.len() < file_size {
        content.extend_from_slice(b"INFO resumable upload\n");
    }
    content.truncate(file_size);
    tokio::fs::write(session_dir.join("input.part"), &content)
        .await
        .unwrap();
    create_session(
        &pool,
        CreateSessionRow {
            id: session_id.into(),
            issue_code: "SESSIONFINAL".into(),
            owner_user_id: user.id.clone(),
            idempotency_key: "final-key".into(),
            file_name: "resumable.log".into(),
            file_size_bytes: file_size as u64,
            last_modified_ms: None,
            chunk_size_bytes: CHUNK_SIZE_BYTES,
            input_path: format!(".uploads/{session_id}/input.part"),
            expires_at: "2099-01-01 00:00:00".into(),
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE upload_sessions SET committed_offset=?, next_chunk_index=? WHERE id=?")
        .bind(file_size as i64)
        .bind(3_i64)
        .bind(session_id)
        .execute(&pool)
        .await
        .unwrap();
    for (index, (offset, size)) in [(0_i64, 8 * 1024 * 1024_i64), (1, 8 * 1024 * 1024), (2, 1)]
        .into_iter()
        .enumerate()
    {
        sqlx::query(
            "INSERT INTO upload_session_chunks (session_id, chunk_index, offset_bytes, size_bytes, sha256) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(session_id)
        .bind(index as i64)
        .bind(offset)
        .bind(size)
        .bind(format!("{index:064x}"))
        .execute(&pool)
        .await
        .unwrap();
    }
    let mut limits = AppLimits::default();
    limits.upload.concurrent_processing_tasks = 1;
    let state = web::Data::new(AppState::new(pool.clone(), data_root.clone(), limits));
    state
        .upload
        .tmp_bytes
        .store(file_size as u64, std::sync::atomic::Ordering::Release);
    let app = actix_test::init_service(
        App::new()
            .app_data(state.clone())
            .configure(routes::register),
    )
    .await;
    let complete = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri(&format!("/api/upload-sessions/{session_id}/complete"))
            .cookie(Cookie::new(SESSION_COOKIE_NAME, token))
            .to_request(),
    )
    .await;
    assert_eq!(complete.status(), actix_web::http::StatusCode::ACCEPTED);

    backend::upload::session_finalizer::finalize_one(&state, session_id)
        .await
        .unwrap();
    let delivered: (String, String) =
        sqlx::query_as("SELECT status, bundle_id FROM upload_sessions WHERE id=?")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(delivered.0, "DELIVERED");
    let bundle_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bundles WHERE id=?")
        .bind(&delivered.1)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(bundle_count, 1);

    for _ in 0..200 {
        let status: String = sqlx::query_scalar("SELECT status FROM bundles WHERE id=?")
            .bind(&delivered.1)
            .fetch_one(&pool)
            .await
            .unwrap();
        if matches!(status.as_str(), "READY" | "FAILED") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let status: String = sqlx::query_scalar("SELECT status FROM bundles WHERE id=?")
        .bind(&delivered.1)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "READY");

    let reconcile_id = "reconcile-session";
    let reconcile_dir = data_root.join(".uploads").join(reconcile_id);
    tokio::fs::create_dir_all(&reconcile_dir).await.unwrap();
    let reconcile_path = reconcile_dir.join("input.part");
    let reconcile_file = tokio::fs::File::create(&reconcile_path).await.unwrap();
    reconcile_file.set_len(9 * 1024 * 1024).await.unwrap();
    create_session(
        &pool,
        CreateSessionRow {
            id: reconcile_id.into(),
            issue_code: "SESSIONFINAL".into(),
            owner_user_id: user.id,
            idempotency_key: "reconcile-key".into(),
            file_name: "reconcile.log".into(),
            file_size_bytes: 17 * 1024 * 1024,
            last_modified_ms: None,
            chunk_size_bytes: CHUNK_SIZE_BYTES,
            input_path: format!(".uploads/{reconcile_id}/input.part"),
            expires_at: "2099-01-01 00:00:00".into(),
        },
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE upload_sessions SET committed_offset=8388608, next_chunk_index=1 WHERE id=?",
    )
    .bind(reconcile_id)
    .execute(&pool)
    .await
    .unwrap();
    backend::upload::session_finalizer::reconcile_startup(&pool, &data_root)
        .await
        .unwrap();
    assert_eq!(
        tokio::fs::metadata(reconcile_path).await.unwrap().len(),
        8 * 1024 * 1024
    );
    let _ = tokio::fs::remove_dir_all(data_root).await;
}

fn format_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn expired_session_is_terminal_before_input_cleanup_and_capacity_release() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let user = match users::create_user(&pool, "session-expiry-owner", "hash")
        .await
        .unwrap()
    {
        CreateUserOutcome::Created(user) => user,
        CreateUserOutcome::DuplicateUsername => panic!("duplicate test user"),
    };
    sqlx::query("INSERT INTO issues (code, name, owner_user_id) VALUES ('SESSIONEXPIRY', 'Session Expiry', ?)")
        .bind(&user.id)
        .execute(&pool)
        .await
        .unwrap();
    let data_root = std::env::temp_dir().join(format!("rain-upload-session-{}", Uuid::new_v4()));
    let session_dir = data_root.join(".uploads").join("expired-session");
    tokio::fs::create_dir_all(&session_dir).await.unwrap();
    tokio::fs::write(session_dir.join("input.part"), b"expired")
        .await
        .unwrap();
    create_session(
        &pool,
        CreateSessionRow {
            id: "expired-session".into(),
            issue_code: "SESSIONEXPIRY".into(),
            owner_user_id: user.id,
            idempotency_key: "expiry-key".into(),
            file_name: "expired.log".into(),
            file_size_bytes: 64 * 1024 * 1024,
            last_modified_ms: None,
            chunk_size_bytes: CHUNK_SIZE_BYTES,
            input_path: ".uploads/expired-session/input.part".into(),
            expires_at: "2000-01-01 00:00:00".into(),
        },
    )
    .await
    .unwrap();
    let state = AppState::new(pool.clone(), data_root.clone(), AppLimits::default());
    state
        .upload
        .tmp_bytes
        .store(64 * 1024 * 1024, std::sync::atomic::Ordering::Release);
    backend::upload::session_finalizer::run_once(&state)
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM upload_sessions WHERE id=?")
        .bind("expired-session")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "EXPIRED");
    assert_eq!(
        state
            .upload
            .tmp_bytes
            .load(std::sync::atomic::Ordering::Acquire),
        0
    );
    assert!(tokio::fs::metadata(session_dir).await.is_err());
    let _ = tokio::fs::remove_dir_all(data_root).await;
}
