use backend::{
    db,
    repositories::users::{self, CreateUserOutcome},
    upload::session::{
        CHUNK_SIZE_BYTES, CreateSessionRow, create_session, expected_chunk_size,
        find_by_idempotency,
    },
};
use sqlx::sqlite::SqlitePoolOptions;

#[test]
fn expected_chunk_size_accepts_short_final_chunk_and_rejects_overflow() {
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
