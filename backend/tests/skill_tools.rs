use std::path::PathBuf;

use backend::{
    AppState,
    config::AppLimits,
    db,
    services::skill_tools::{SkillRunContext, SkillToolExecutor},
};

#[tokio::test]
async fn list_files_is_bound_to_ready_bundles_in_the_run_issue() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('A','A'),('B','B')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage) VALUES('a-ready','A','ha','a','READY','PUBLISHING'),('a-pending','A','hap','ap','PENDING','RECEIVING'),('b-ready','B','hb','b','READY','PUBLISHING')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('a-ready','visible.log','/visible.log',0),('a-pending','pending.log','/pending.log',0),('b-ready','foreign.log','/foreign.log',0)")
        .execute(&pool).await.unwrap();
    let state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());
    let mut executor = SkillToolExecutor::new(
        &state,
        SkillRunContext {
            run_id: "run".into(),
            user_id: "user".into(),
            issue_code: "A".into(),
        },
    );

    let files = executor.list_files().await.unwrap();
    let text = files.to_string();
    assert!(text.contains("visible.log"));
    assert!(!text.contains("pending.log"));
    assert!(!text.contains("foreign.log"));
}
