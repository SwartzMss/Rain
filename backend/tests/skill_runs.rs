use backend::{db, models::skill_runs::NewSkillRun, repositories::skill_runs};

#[tokio::test]
async fn run_state_is_atomic_concurrent_and_temporary() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('u','user','user','hash'),('v','viewer','viewer','hash')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('ISSUE','Issue')")
        .execute(&pool)
        .await
        .unwrap();
    let new_run = NewSkillRun {
        user_id: "u".into(),
        issue_code: "ISSUE".into(),
        skill_id: "skill".into(),
        skill_version: 1,
        skill_name: "Skill".into(),
        skill_snapshot_markdown: "# Skill".into(),
    };
    let first = skill_runs::create(&pool, &new_run).await.unwrap();
    assert!(skill_runs::create(&pool, &new_run).await.is_err());
    assert!(skill_runs::mark_running(&pool, &first.id).await.unwrap());
    assert!(skill_runs::cancel(&pool, &first.id, "u").await.unwrap());
    assert!(
        !skill_runs::complete(&pool, &first.id, "{\"summary\":\"late\"}")
            .await
            .unwrap()
    );

    sqlx::query("UPDATE skill_runs SET completed_at=datetime('now','-25 hours') WHERE id=?")
        .bind(&first.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        skill_runs::cleanup_expired(&pool, 24 * 60 * 60)
            .await
            .unwrap(),
        1
    );
}
