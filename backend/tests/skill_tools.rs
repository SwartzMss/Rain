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
    sqlx::query("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('a-ready','visible.log','/visible.log',0),('a-ready','archive.zip','/archive.zip',0),('a-ready','logs','/logs',1),('a-pending','pending.log','/pending.log',0),('b-ready','foreign.log','/foreign.log',0)")
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

    let files = executor.list_files(None, None).await.unwrap();
    let text = files.to_string();
    assert!(text.contains("visible.log"));
    assert!(text.contains("\"bundle_hash\":\"ha\""));
    let directory = files["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["path"] == "/logs")
        .unwrap();
    assert_eq!(directory["is_dir"], true);
    let archive = files["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["path"] == "/archive.zip")
        .unwrap();
    assert_eq!(archive["preview_kind"], "archive");
    assert_eq!(archive["text_readable"], false);
    assert!(!text.contains("pending.log"));
    assert!(!text.contains("foreign.log"));

    let directory_read = executor
        .read_file_lines(directory["file_id"].as_i64().unwrap(), 0, 10)
        .await
        .unwrap();
    assert_eq!(directory_read["error"], "FILE_IS_DIRECTORY");
    assert_eq!(directory_read["lines"], serde_json::json!([]));

    let archive_read = executor
        .read_file_lines(archive["file_id"].as_i64().unwrap(), 0, 10)
        .await
        .unwrap();
    assert_eq!(archive_read["error"], "FILE_NOT_TEXT");
    assert_eq!(archive_read["lines"], serde_json::json!([]));
}

#[tokio::test]
async fn list_files_can_continue_with_a_cursor() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('A','A')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage) VALUES('bundle','A','hash-a','a','READY','PUBLISHING')").execute(&pool).await.unwrap();
    for index in 0..600 {
        sqlx::query("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('bundle',?,?,0)")
            .bind(format!("{index:04}.log"))
            .bind(format!("/{index:04}-{}.log", "x".repeat(100)))
            .execute(&pool)
            .await
            .unwrap();
    }
    let state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());
    let mut executor = SkillToolExecutor::new(
        &state,
        SkillRunContext {
            run_id: "run".into(),
            user_id: "user".into(),
            issue_code: "A".into(),
        },
    );

    let first = executor.list_files(None, None).await.unwrap();
    let cursor = first["next_cursor"].as_i64().expect("first page cursor");
    let second = executor.list_files(Some(cursor), None).await.unwrap();

    assert!(
        second["files"]
            .as_array()
            .is_some_and(|files| !files.is_empty())
    );
    assert!(second["files"][0]["file_id"].as_i64().unwrap() > cursor);
}

#[tokio::test]
async fn issue_manifest_is_bounded_to_ready_bundles_and_does_not_record_evidence() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('A','A'),('B','B')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage,content_size_bytes) VALUES('a-ready','A','ha','android.zip','READY','PUBLISHING',300),('a-pending','A','hap','pending.zip','PENDING','RECEIVING',900),('b-ready','B','hb','foreign.zip','READY','PUBLISHING',700)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO files(bundle_id,name,path,is_dir,size_bytes,line_count) VALUES('a-ready','boot.log','/android/boot.log',0,200,20),('a-ready','config.txt','/android/config.txt',0,100,NULL),('a-ready','android','/android',1,NULL,NULL),('a-pending','hidden.log','/pending/hidden.log',0,999,99),('b-ready','foreign.log','/foreign/foreign.log',0,700,70)")
        .execute(&pool)
        .await
        .unwrap();
    let state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());
    let mut executor = SkillToolExecutor::new(
        &state,
        SkillRunContext {
            run_id: "run".into(),
            user_id: "user".into(),
            issue_code: "A".into(),
        },
    );

    let manifest = executor.get_issue_manifest().await.unwrap();
    assert_eq!(manifest["issue"]["bundle_count"], 2);
    assert_eq!(manifest["issue"]["ready_bundle_count"], 1);
    assert_eq!(manifest["issue"]["file_count"], 2);
    assert_eq!(manifest["issue"]["directory_count"], 1);
    assert_eq!(manifest["issue"]["indexed_text_file_count"], 1);
    assert_eq!(manifest["issue"]["total_content_bytes"], 300);
    assert_eq!(manifest["bundles"].as_array().unwrap().len(), 1);
    assert_eq!(manifest["bundles"][0]["hash"], "ha");
    assert_eq!(manifest["extensions"][0]["extension"], ".log");
    assert_eq!(manifest["top_path_prefixes"][0]["prefix"], "/android");
    assert_eq!(manifest["largest_files"][0]["path"], "/android/boot.log");
    assert!(!manifest.to_string().contains("pending"));
    assert!(!manifest.to_string().contains("foreign"));
    assert!(executor.ledger.evidence().is_empty());
    assert_eq!(executor.ledger.total_bytes(), 0);
    assert!(serde_json::to_vec(&manifest).unwrap().len() <= 32 * 1024);
}

#[tokio::test]
async fn issue_manifest_limits_large_result_sets() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('A','A')")
        .execute(&pool)
        .await
        .unwrap();
    for index in 0..60 {
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage,content_size_bytes) VALUES(?,?,?,?,?,?,?)")
            .bind(format!("bundle-{index}"))
            .bind("A")
            .bind(format!("hash-{index:03}"))
            .bind(format!("bundle-{index}.zip"))
            .bind("READY")
            .bind("PUBLISHING")
            .bind(1_i64)
            .execute(&pool)
            .await
            .unwrap();
    }
    let state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());
    let mut executor = SkillToolExecutor::new(
        &state,
        SkillRunContext {
            run_id: "run".into(),
            user_id: "user".into(),
            issue_code: "A".into(),
        },
    );
    let manifest = executor.get_issue_manifest().await.unwrap();
    assert_eq!(manifest["issue"]["ready_bundle_count"], 60);
    assert_eq!(manifest["truncated"], true);
    assert!(manifest["bundles"].as_array().unwrap().len() <= 50);
    assert!(serde_json::to_vec(&manifest).unwrap().len() <= 32 * 1024);
}
