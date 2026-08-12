use std::path::PathBuf;

use backend::{
    AppState,
    config::AppLimits,
    db,
    error::AppError,
    services::skill_tools::{SkillRunContext, SkillToolExecutor},
};
use uuid::Uuid;

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
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap().len();
    assert_eq!(executor.ledger.total_bytes(), manifest_bytes);
    assert!(manifest_bytes <= 32 * 1024);

    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage,content_size_bytes) VALUES('a-later','A','hz','late.zip','READY','PUBLISHING',500)")
        .execute(&state.db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO files(bundle_id,name,path,is_dir,size_bytes,line_count) VALUES('a-later','late.log','/late.log',0,500,50)")
        .execute(&state.db.pool)
        .await
        .unwrap();
    let cached = executor.get_issue_manifest().await.unwrap();
    assert_eq!(cached, manifest);
    assert_eq!(cached["issue"]["ready_bundle_count"], 1);
    assert!(!cached.to_string().contains("late.log"));
    assert_eq!(executor.ledger.total_bytes(), manifest_bytes * 2);
    assert!(executor.ledger.evidence().is_empty());
}

#[tokio::test]
async fn issue_manifest_groups_last_extensions_and_root_level_paths() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('A','A')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage) VALUES('bundle','A','hash','logs','READY','PUBLISHING')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('bundle','boot.log','/boot.log',0),('bundle','system.log','/system.log',0),('bundle','qsee.log','/qsee.log',0),('bundle','app.1.log','/logs/app.1.log',0),('bundle','app.2.log','/logs/app.2.log',0),('bundle','archive.tar.gz','/logs/archive.tar.gz',0)")
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
    let extensions = manifest["extensions"].as_array().unwrap();
    assert_eq!(extensions[0]["extension"], ".log");
    assert_eq!(extensions[0]["file_count"], 5);
    assert!(
        extensions
            .iter()
            .any(|item| { item["extension"] == ".gz" && item["file_count"] == 1 })
    );
    let prefixes = manifest["top_path_prefixes"].as_array().unwrap();
    assert!(
        prefixes
            .iter()
            .any(|item| { item["prefix"] == "/" && item["file_count"] == 3 })
    );
    assert!(
        prefixes
            .iter()
            .any(|item| { item["prefix"] == "/logs" && item["file_count"] == 3 })
    );
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
            .bind(format!("{}-{index}.zip", "x".repeat(500)))
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
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap().len();
    assert_eq!(manifest["issue"]["ready_bundle_count"], 60);
    assert_eq!(manifest["truncated"], true);
    assert!(manifest["bundles"].as_array().unwrap().len() <= 50);
    assert!(manifest_bytes <= 32 * 1024);
    assert!(manifest_bytes > 128 * 1024 / 24);

    let mut successful_calls = 1;
    let mut limited = false;
    for _ in 1..24 {
        match executor.get_issue_manifest().await {
            Ok(_) => successful_calls += 1,
            Err(AppError::BadRequest(message))
                if message.contains("retrieval byte limit reached") =>
            {
                limited = true;
                break;
            }
            Err(error) => panic!("unexpected manifest error: {error}"),
        }
    }
    assert!(
        limited,
        "24 manifest calls must not bypass the shared budget"
    );
    assert!(successful_calls < 24);
    assert_eq!(
        executor.ledger.total_bytes(),
        manifest_bytes * successful_calls
    );
    assert!(executor.ledger.total_bytes() <= 128 * 1024);
    assert!(executor.ledger.evidence().is_empty());
}

#[tokio::test]
async fn search_logs_supports_scoped_filters_short_terms_and_bounded_snippets() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('A','A'),('B','B')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage) VALUES('a-one','A','hash-a','a','READY','PUBLISHING'),('a-two','A','hash-a2','a2','READY','PUBLISHING'),('a-pending','A','hash-p','pending','PENDING','RECEIVING'),('b-one','B','hash-b','b','READY','PUBLISHING')")
        .execute(&pool).await.unwrap();

    let file_a: i64 = sqlx::query_scalar("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('a-one','app.log','/qnx/app.log',0) RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let file_a2: i64 = sqlx::query_scalar("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('a-two','other.log','/android/other.log',0) RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let wildcard_file: i64 = sqlx::query_scalar("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('a-one','literal.log','/literal%_\\/literal.log',0) RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let wildcard_decoy: i64 = sqlx::query_scalar("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('a-one','decoy.log','/literalXX/decoy.log',0) RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let pending_file: i64 = sqlx::query_scalar("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('a-pending','pending.log','/qnx/pending.log',0) RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let foreign_file: i64 = sqlx::query_scalar("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('b-one','foreign.log','/qnx/foreign.log',0) RETURNING id")
        .fetch_one(&pool).await.unwrap();

    let middle_content = format!(
        "{} COFACTOR failed rv ÄBC failure {}",
        "0123456789abcdef\n".repeat(30_000),
        "after ".repeat(100)
    );
    for (bundle, file_id, content, line) in [
        ("a-one", file_a, middle_content.as_str(), 10_i64),
        ("a-two", file_a2, "timeout and rv in android", 20),
        ("a-one", wildcard_file, "wildmarker in literal path", 30),
        ("a-one", wildcard_decoy, "wildmarker in decoy path", 40),
        ("a-pending", pending_file, "timeout hidden pending", 50),
        ("b-one", foreign_file, "timeout hidden foreign rv", 60),
    ] {
        sqlx::query("INSERT INTO log_segments(bundle_id,file_id,content,line_offset,line_end,chunk_index) VALUES(?,?,?,?,?,0)")
            .bind(bundle)
            .bind(file_id)
            .bind(content)
            .bind(line)
            .bind(line + 5)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO log_segments(bundle_id,file_id,content,line_offset,line_end,chunk_index) VALUES('a-one',?,?,70,70,1)")
        .bind(file_a)
        .bind("x".repeat(200))
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

    let fts = executor
        .search_logs("cofactor", Some("/qnx"), Some("hash-a"), Some(file_a))
        .await
        .unwrap();
    assert_eq!(fts["search_mode"], "fts");
    assert_eq!(fts["hits"].as_array().unwrap().len(), 1);
    assert_eq!(fts["hits"][0]["file_id"], file_a);
    let snippet = fts["hits"][0]["snippet"].as_str().unwrap();
    assert!(snippet.to_ascii_lowercase().contains("cofactor"));
    assert!(snippet.len() <= 400);
    assert_eq!(
        executor.ledger.total_bytes(),
        serde_json::to_vec(&fts).unwrap().len()
    );

    let unicode_fts = executor
        .search_logs("äbc", None, None, Some(file_a))
        .await
        .unwrap();
    let unicode_snippet = unicode_fts["hits"][0]["snippet"].as_str().unwrap();
    assert!(unicode_snippet.contains("ÄBC"));
    assert!(unicode_snippet.len() <= 400);

    let short = executor
        .search_logs("rv", None, None, Some(file_a))
        .await
        .unwrap();
    assert_eq!(short["search_mode"], "short_literal");
    assert_eq!(short["hits"].as_array().unwrap().len(), 1);
    assert!(short["hits"][0]["snippet"].as_str().unwrap().contains("rv"));
    assert_eq!(
        executor.ledger.total_bytes(),
        serde_json::to_vec(&fts).unwrap().len()
            + serde_json::to_vec(&unicode_fts).unwrap().len()
            + serde_json::to_vec(&short).unwrap().len()
    );
    assert!(executor.search_logs("rv", None, None, None).await.is_err());
    assert!(
        executor
            .search_logs("r", None, None, Some(file_a))
            .await
            .is_err()
    );
    assert!(
        executor
            .search_logs(&"x".repeat(201), None, None, None)
            .await
            .is_err()
    );
    let boundary = executor
        .search_logs(&"x".repeat(200), None, None, Some(file_a))
        .await
        .unwrap();
    assert_eq!(boundary["hits"].as_array().unwrap().len(), 1);

    let other_file = executor
        .search_logs("timeout", None, None, Some(file_a2))
        .await
        .unwrap();
    assert_eq!(other_file["hits"].as_array().unwrap().len(), 1);
    let wrong_bundle = executor
        .search_logs("timeout", None, Some("hash-a"), Some(file_a2))
        .await
        .unwrap();
    assert!(wrong_bundle["hits"].as_array().unwrap().is_empty());
    let foreign = executor
        .search_logs("timeout", None, None, Some(foreign_file))
        .await
        .unwrap();
    assert!(foreign["hits"].as_array().unwrap().is_empty());
    let pending = executor
        .search_logs("timeout", None, None, Some(pending_file))
        .await
        .unwrap();
    assert!(pending["hits"].as_array().unwrap().is_empty());

    let escaped_prefix = executor
        .search_logs("wildmarker", Some("/literal%_\\"), None, None)
        .await
        .unwrap();
    assert_eq!(escaped_prefix["hits"].as_array().unwrap().len(), 1);
    assert_eq!(escaped_prefix["hits"][0]["file_id"], wildcard_file);
    assert_ne!(escaped_prefix["hits"][0]["file_id"], wildcard_decoy);
    assert!(executor.ledger.evidence().is_empty());
    assert!(executor.ledger.total_bytes() <= 128 * 1024);
}

#[tokio::test]
async fn search_logs_caps_hits_and_marks_truncation() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('A','A')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage) VALUES('a','A','hash-a','a','READY','PUBLISHING')")
        .execute(&pool).await.unwrap();
    let file_id: i64 = sqlx::query_scalar("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('a','app.log','/app.log',0) RETURNING id")
        .fetch_one(&pool).await.unwrap();
    for index in 0..25_i64 {
        sqlx::query("INSERT INTO log_segments(bundle_id,file_id,content,line_offset,line_end,chunk_index) VALUES('a',?,'repeated marker',?,?,?)")
            .bind(file_id)
            .bind(index)
            .bind(index)
            .bind(index)
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

    let result = executor
        .search_logs("marker", None, None, None)
        .await
        .unwrap();
    assert_eq!(result["hits"].as_array().unwrap().len(), 20);
    assert_eq!(result["truncated"], true);
    assert!(serde_json::to_vec(&result).unwrap().len() <= 32 * 1024);
    assert!(executor.ledger.evidence().is_empty());
}

#[tokio::test]
async fn search_logs_duplicate_key_includes_normalized_filters() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('A','A')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage) VALUES('a','A','hash-a','a','READY','PUBLISHING')")
        .execute(&pool).await.unwrap();
    let file_id: i64 = sqlx::query_scalar("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('a','app.log','/qnx/app.log',0) RETURNING id")
        .fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO log_segments(bundle_id,file_id,content,line_offset,line_end,chunk_index) VALUES('a',?,'timeout happened ÄB failure',1,1,0)")
        .bind(file_id).execute(&pool).await.unwrap();
    let state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());
    let mut executor = SkillToolExecutor::new(
        &state,
        SkillRunContext {
            run_id: "run".into(),
            user_id: "user".into(),
            issue_code: "A".into(),
        },
    );

    executor
        .search_logs(" timeout ", Some(" /qnx "), Some("hash-a"), Some(file_id))
        .await
        .unwrap();
    let duplicate = executor
        .search_logs("TIMEOUT", Some("/QNX"), Some("HASH-A"), Some(file_id))
        .await
        .unwrap();
    assert_eq!(duplicate["duplicate"], true);
    let different_path = executor
        .search_logs("timeout", Some("/android"), Some("hash-a"), Some(file_id))
        .await
        .unwrap();
    assert_ne!(different_path["duplicate"], true);
    let different_bundle = executor
        .search_logs("timeout", Some("/qnx"), Some("other"), Some(file_id))
        .await
        .unwrap();
    assert_ne!(different_bundle["duplicate"], true);
    let different_file = executor
        .search_logs("timeout", Some("/qnx"), Some("hash-a"), None)
        .await
        .unwrap();
    assert_ne!(different_file["duplicate"], true);

    let unicode_lower = executor
        .search_logs("äb", None, None, Some(file_id))
        .await
        .unwrap();
    assert!(unicode_lower["hits"].as_array().unwrap().is_empty());
    let unicode_exact = executor
        .search_logs("ÄB", None, None, Some(file_id))
        .await
        .unwrap();
    assert_ne!(unicode_exact["duplicate"], true);
    assert_eq!(unicode_exact["hits"].as_array().unwrap().len(), 1);

    let separator_in_path = executor
        .search_logs("timeout", Some("/qnx\u{1f}hash-a"), None, Some(file_id))
        .await
        .unwrap();
    assert_ne!(separator_in_path["duplicate"], true);
    let separator_in_bundle = executor
        .search_logs("timeout", Some("/qnx"), Some("hash-a\u{1f}"), Some(file_id))
        .await
        .unwrap();
    assert_ne!(separator_in_bundle["duplicate"], true);
}

#[tokio::test]
async fn read_file_lines_exposes_bounded_long_lines_and_records_only_returned_evidence() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('A','A')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage) VALUES('a','A','hash-a','a','READY','PUBLISHING')")
        .execute(&pool)
        .await
        .unwrap();

    let first_line = format!(
        "{}KEY_AFTER_128{}TAIL_SECRET",
        "a".repeat(200),
        "b".repeat(4_300)
    );
    let utf8_line = "界".repeat(2_000);
    let mut source_lines = vec![first_line.clone(), utf8_line.clone()];
    for line in 2..16 {
        source_lines.push(format!(
            "line-{line}-{}-DROPPED_MARKER_{line}",
            "z".repeat(5_000)
        ));
    }
    source_lines.push("short complete".into());
    let source = source_lines.join("\r\n");
    let data_root =
        std::env::temp_dir().join(format!("rain-skill-lines-{}", Uuid::new_v4().simple()));
    let storage_key = "blobs/te/test-long-lines";
    let blob_path = data_root.join(storage_key);
    tokio::fs::create_dir_all(blob_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&blob_path, source.as_bytes())
        .await
        .unwrap();
    let blob_id: i64 = sqlx::query_scalar("INSERT INTO blobs(content_hash,size_bytes,storage_backend,storage_key,state) VALUES('test-long-lines',?,'local',?,'READY') RETURNING id")
        .bind(source.len() as i64)
        .bind(storage_key)
        .fetch_one(&pool)
        .await
        .unwrap();
    let file_id: i64 = sqlx::query_scalar("INSERT INTO files(bundle_id,name,path,is_dir,size_bytes,line_count,mime_type,blob_id) VALUES('a','long.log','/long.log',0,?,17,'text/plain',?) RETURNING id")
        .bind(source.len() as i64)
        .bind(blob_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let state = AppState::new(pool, data_root.clone(), AppLimits::default());
    let mut executor = SkillToolExecutor::new(
        &state,
        SkillRunContext {
            run_id: "run".into(),
            user_id: "user".into(),
            issue_code: "A".into(),
        },
    );
    assert!(executor.read_file_lines(0, 0, 1).await.is_err());
    assert!(executor.read_file_lines(file_id, 0, 201).await.is_err());
    assert!(executor.read_file_lines(file_id, 5, 0).await.is_err());
    assert!(
        executor
            .read_file_lines(file_id, i64::MAX, 2)
            .await
            .is_err()
    );

    let result = executor.read_file_lines(file_id, 0, 15).await.unwrap();
    let result_bytes = serde_json::to_vec(&result).unwrap();
    let lines = result["lines"].as_array().unwrap();
    assert!(result_bytes.len() <= 32 * 1024);
    assert_eq!(result["truncated"], true);
    assert!(!lines.is_empty());
    assert!(lines.len() < source_lines.len());
    assert_eq!(lines[0]["truncated"], true);
    assert_eq!(lines[0]["original_length"], first_line.len());
    let first_content = lines[0]["content"].as_str().unwrap();
    assert!(first_content.contains("KEY_AFTER_128"));
    assert!(!first_content.contains("TAIL_SECRET"));
    assert!(first_content.len() <= 4 * 1024 + " ... [line truncated]".len());
    assert_eq!(lines[1]["truncated"], true);
    assert_eq!(lines[1]["original_length"], utf8_line.len());
    assert!(!lines[1]["content"].as_str().unwrap().contains('\u{fffd}'));

    assert!(executor.ledger.supports_evidence(
        "hash-a",
        file_id,
        "/long.log",
        0,
        0,
        "KEY_AFTER_128"
    ));
    assert!(!executor.ledger.supports_evidence(
        "hash-a",
        file_id,
        "/long.log",
        0,
        0,
        "TAIL_SECRET"
    ));
    let last_returned = lines.last().unwrap()["line_number"].as_i64().unwrap();
    let first_dropped = last_returned + 1;
    assert!(!executor.ledger.supports_evidence(
        "hash-a",
        file_id,
        "/long.log",
        first_dropped,
        first_dropped,
        &format!("DROPPED_MARKER_{first_dropped}")
    ));

    let max_range = executor.read_file_lines(file_id, 16, 200).await.unwrap();
    assert_eq!(max_range["lines"][0]["content"], "short complete");
    assert_eq!(max_range["lines"][0]["truncated"], false);
    assert!(max_range["lines"][0].get("original_length").is_none());

    let duplicate = executor
        .read_file_lines(file_id, 0, last_returned + 1)
        .await
        .unwrap();
    assert_eq!(duplicate["duplicate"], true);
    let continuation = executor.read_file_lines(file_id, 0, 15).await.unwrap();
    assert_eq!(continuation["lines"][0]["line_number"], first_dropped);

    tokio::fs::remove_dir_all(data_root).await.unwrap();
}
