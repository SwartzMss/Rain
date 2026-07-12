use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use actix_web::{App, http::StatusCode, test, web};
use backend::{AppState, db, routes};
use flate2::{Compression, write::GzEncoder};
use serde_json::Value;
use uuid::Uuid;

#[actix_web::test]
async fn upload_search_tree_and_delete_issue() {
    let test_dir = TestDir::new("rain-smoke");
    let db_url = sqlite_url(&test_dir.path.join("rain.db"));
    let data_root = test_dir.path.join("uploads");
    fs::create_dir_all(&data_root).expect("create data root");

    let pool = db::init_pool(&db_url).expect("init sqlite pool");
    db::prepare_schema(&pool, true)
        .await
        .expect("prepare schema");
    let app_pool = pool.clone();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppState {
                pool: app_pool,
                data_root,
            }))
            .configure(routes::register),
    )
    .await;

    let boundary = format!("rain-{}", Uuid::new_v4().simple());
    let upload_body = multipart_body(
        &boundary,
        "SMOKE",
        "app.log",
        "INFO boot\nERROR smoke works\n",
    );
    let upload_response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            ))
            .set_payload(upload_body)
            .to_request(),
    )
    .await;
    assert_eq!(upload_response.status(), StatusCode::ACCEPTED);
    let upload: Value = test::read_body_json(upload_response).await;
    let bundle_hash = upload
        .get("bundle_hash")
        .and_then(Value::as_str)
        .expect("bundle hash");
    assert_eq!(upload["task_id"], bundle_hash);
    assert_eq!(upload["status"], "PROCESSING");
    assert_eq!(upload["issue_code"], "SMOKE");
    let task_status: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/uploads/{bundle_hash}"))
            .to_request(),
    )
    .await;
    assert_eq!(task_status["task_id"], bundle_hash);
    wait_for_issue_ready(&pool, "SMOKE").await;

    let search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/SMOKE/search?q=smoke&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(search["total"], 1);
    assert!(
        search["hits"][0]["snippet"]
            .as_str()
            .expect("snippet")
            .contains("ERROR smoke works")
    );
    assert_eq!(search["hits"][0]["line_number"], 0);
    assert_eq!(search["hits"][0]["line_end"], 1);
    assert_eq!(search["hits"][0]["chunk_index"], 0);

    let gz_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let gz_bytes = gzip_bytes("INFO gzip\nERROR compressed smoke works\n");
    let gz_upload_body = multipart_body_bytes(
        &gz_boundary,
        "GZIP",
        "compressed.log.gz",
        "application/gzip",
        &gz_bytes,
    );
    let gz_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={gz_boundary}"),
            ))
            .set_payload(gz_upload_body)
            .to_request(),
    )
    .await;
    assert_eq!(gz_upload["issue_code"], "GZIP");
    wait_for_issue_ready(&pool, "GZIP").await;

    let gz_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/GZIP/search?q=compressed&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(gz_search["total"], 1);
    assert!(
        gz_search["hits"][0]["snippet"]
            .as_str()
            .expect("gzip snippet")
            .contains("ERROR compressed smoke works")
    );

    let tar_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let tar_bytes = tar_gz_bytes("nested/service.log", "INFO tar\nERROR targz smoke works\n");
    let tar_upload_body = multipart_body_bytes(
        &tar_boundary,
        "TARGZ",
        "logs.tar.gz",
        "application/gzip",
        &tar_bytes,
    );
    let tar_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={tar_boundary}"),
            ))
            .set_payload(tar_upload_body)
            .to_request(),
    )
    .await;
    assert_eq!(tar_upload["issue_code"], "TARGZ");
    wait_for_issue_ready(&pool, "TARGZ").await;

    let tar_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/TARGZ/search?q=targz&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(tar_search["total"], 1);
    assert!(
        tar_search["hits"][0]["snippet"]
            .as_str()
            .expect("tar snippet")
            .contains("ERROR targz smoke works")
    );

    let delete_dir_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let delete_dir_bytes = tar_gz_multi(&[
        ("a_b/target.log", "ERROR delete only this directory\n"),
        ("axb/keep.log", "ERROR keep this directory\n"),
    ]);
    let delete_dir_upload_body = multipart_body_bytes(
        &delete_dir_boundary,
        "DIRDELETE",
        "delete-dirs.tar.gz",
        "application/gzip",
        &delete_dir_bytes,
    );
    let delete_dir_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={delete_dir_boundary}"),
            ))
            .set_payload(delete_dir_upload_body)
            .to_request(),
    )
    .await;
    let delete_dir_bundle = delete_dir_upload["bundle_hash"]
        .as_str()
        .expect("delete dir bundle");
    wait_for_issue_ready(&pool, "DIRDELETE").await;
    let delete_dir_root: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/files/v1/{delete_dir_bundle}/files/root"))
            .to_request(),
    )
    .await;
    let archive_id = delete_dir_root["children"][0]["id"]
        .as_str()
        .expect("archive id");
    let archive_node: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/files/v1/{delete_dir_bundle}/files/{archive_id}"
            ))
            .to_request(),
    )
    .await;
    let extracted_id = archive_node["children"][0]["id"]
        .as_str()
        .expect("extracted id");
    let extracted_node: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/files/v1/{delete_dir_bundle}/files/{extracted_id}"
            ))
            .to_request(),
    )
    .await;
    let dirs = extracted_node["children"].as_array().expect("dirs");
    let target_dir_id = dirs
        .iter()
        .find(|node| node["name"] == "a_b")
        .and_then(|node| node["id"].as_str())
        .expect("a_b dir");
    assert!(dirs.iter().any(|node| node["name"] == "axb"));
    let delete_dir_response = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!(
                "/api/files/v1/{delete_dir_bundle}/files/{target_dir_id}"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(delete_dir_response.status(), StatusCode::NO_CONTENT);
    let extracted_after_delete: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/files/v1/{delete_dir_bundle}/files/{extracted_id}"
            ))
            .to_request(),
    )
    .await;
    let remaining_dirs = extracted_after_delete["children"]
        .as_array()
        .expect("remaining dirs");
    assert!(remaining_dirs.iter().any(|node| node["name"] == "axb"));
    assert!(!remaining_dirs.iter().any(|node| node["name"] == "a_b"));

    let failed_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let failed_bytes = tar_gz_multi(&[
        ("dup/path.log", "ERROR first duplicate\n"),
        ("dup/path.log", "ERROR second duplicate\n"),
    ]);
    let failed_upload_body = multipart_body_bytes(
        &failed_boundary,
        "FAILEDCASE",
        "bad-archive.tar.gz",
        "application/gzip",
        &failed_bytes,
    );
    let failed_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={failed_boundary}"),
            ))
            .set_payload(failed_upload_body)
            .to_request(),
    )
    .await;
    let failed_bundle = failed_upload["bundle_hash"]
        .as_str()
        .expect("failed bundle");
    wait_for_issue_status(&pool, "FAILEDCASE", "FAILED").await;
    let failed_task: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/uploads/{failed_bundle}"))
            .to_request(),
    )
    .await;
    assert_eq!(failed_task["status"], "FAILED");
    let failed_tree = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/files/v1/{failed_bundle}/files/root"))
            .to_request(),
    )
    .await;
    assert_eq!(failed_tree.status(), StatusCode::CONFLICT);

    let parsed_events: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM log_events
        WHERE level = 'ERROR' AND message LIKE '%smoke works%'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("count parsed log events");
    assert_eq!(parsed_events, 3);

    let tree: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/files/v1/{bundle_hash}/files/root"))
            .to_request(),
    )
    .await;
    assert_eq!(tree["children"].as_array().expect("children").len(), 1);
    assert_eq!(tree["children"][0]["meta"]["kind"], "uploaded_file");
    assert_eq!(tree["children"][0]["meta"]["line_count"], 2);
    let app_file_id = tree["children"][0]["id"].as_str().expect("file id");

    let lines: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/files/v1/{bundle_hash}/files/{app_file_id}/lines?start=1&limit=1"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(lines["line_count"], 2);
    assert_eq!(lines["lines"][0]["line_number"], 1);
    assert_eq!(lines["lines"][0]["content"], "ERROR smoke works");

    let download = test::call_and_read_body(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/files/v1/{bundle_hash}/files/{app_file_id}/download"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(download, "INFO boot\nERROR smoke works\n");

    let large_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let mut large_log = String::new();
    for line in 0..2_500 {
        large_log.push_str(&format!("INFO filler line {line}\n"));
    }
    large_log.push_str("ERROR tail failure is searchable\n");
    let large_upload_body = multipart_body(&large_boundary, "LARGE", "large.log", &large_log);
    let large_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={large_boundary}"),
            ))
            .set_payload(large_upload_body)
            .to_request(),
    )
    .await;
    assert_eq!(large_upload["issue_code"], "LARGE");
    wait_for_issue_ready(&pool, "LARGE").await;

    let large_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/LARGE/search?q=tail%20failure&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(large_search["total"], 1);
    assert!(
        large_search["hits"][0]["snippet"]
            .as_str()
            .expect("large snippet")
            .contains("ERROR tail failure is searchable")
    );

    let bad_utf8_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let bad_utf8_body = multipart_body_bytes(
        &bad_utf8_boundary,
        "BADUTF8",
        "bad-utf8.log",
        "text/plain",
        b"INFO valid\nERROR invalid byte \xff still indexed\n",
    );
    let bad_utf8_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={bad_utf8_boundary}"),
            ))
            .set_payload(bad_utf8_body)
            .to_request(),
    )
    .await;
    assert_eq!(bad_utf8_upload["issue_code"], "BADUTF8");
    wait_for_issue_ready(&pool, "BADUTF8").await;
    let bad_utf8_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/BADUTF8/search?q=invalid%20byte&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(bad_utf8_search["total"], 1);

    let long_line_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let mut long_line = vec![b'a'; 1024 * 1024 + 128];
    long_line.extend_from_slice(b"\nERROR after long line\n");
    let long_line_body = multipart_body_bytes(
        &long_line_boundary,
        "LONGLINE",
        "long-line.log",
        "text/plain",
        &long_line,
    );
    let long_line_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={long_line_boundary}"),
            ))
            .set_payload(long_line_body)
            .to_request(),
    )
    .await;
    let long_line_bundle = long_line_upload["bundle_hash"]
        .as_str()
        .expect("long line bundle");
    wait_for_issue_ready(&pool, "LONGLINE").await;
    let long_line_tree: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/files/v1/{long_line_bundle}/files/root"))
            .to_request(),
    )
    .await;
    let long_line_file = long_line_tree["children"][0]["id"]
        .as_str()
        .expect("long line file id");
    let long_line_lines: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/files/v1/{long_line_bundle}/files/{long_line_file}/lines?start=0&limit=1"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(long_line_lines["lines"][0]["truncated"], true);
    assert!(
        long_line_lines["lines"][0]["content"]
            .as_str()
            .expect("long line content")
            .ends_with("[line truncated]")
    );

    let collision_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let collision_upload_body = multipart_body_multi(
        &collision_boundary,
        "COLLISION",
        &[
            ("a b.log", "INFO first normalized name\n"),
            ("a?b.log", "ERROR second normalized name\n"),
        ],
    );
    let collision_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={collision_boundary}"),
            ))
            .set_payload(collision_upload_body)
            .to_request(),
    )
    .await;
    let collision_bundle = collision_upload["bundle_hash"]
        .as_str()
        .expect("collision bundle");
    wait_for_issue_ready(&pool, "COLLISION").await;
    let collision_tree: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/files/v1/{collision_bundle}/files/root"))
            .to_request(),
    )
    .await;
    let collision_files = collision_tree["children"].as_array().expect("children");
    assert_eq!(collision_files.len(), 2);
    assert_eq!(collision_files[0]["name"], "a_b.log");
    assert_eq!(collision_files[1]["name"], "a_b.log");
    let first_storage = collision_files[0]["meta"]["storage_name"]
        .as_str()
        .expect("first storage");
    let second_storage = collision_files[1]["meta"]["storage_name"]
        .as_str()
        .expect("second storage");
    assert_ne!(first_storage, second_storage);

    let delete_response = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/issues/SMOKE")
            .to_request(),
    )
    .await;
    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    let missing_response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/SMOKE")
            .to_request(),
    )
    .await;
    assert_eq!(missing_response.status(), StatusCode::NOT_FOUND);
}

#[actix_web::test]
async fn prepare_schema_merges_legacy_issue_code_case_variants() {
    let test_dir = TestDir::new("rain-issue-migrate");
    let db_url = sqlite_url(&test_dir.path.join("rain.db"));
    let pool = db::init_pool(&db_url).expect("init sqlite pool");
    db::prepare_schema(&pool, true)
        .await
        .expect("prepare schema");

    sqlx::query("INSERT INTO issues (code, name) VALUES (?, ?)")
        .bind("cn013")
        .bind("lower")
        .execute(&pool)
        .await
        .expect("insert lower issue");
    sqlx::query("INSERT INTO issues (code, name) VALUES (?, ?)")
        .bind("CN013")
        .bind("upper")
        .execute(&pool)
        .await
        .expect("insert upper issue");
    sqlx::query(
        "INSERT INTO bundles (id, issue_code, hash, name, status) VALUES (?, ?, ?, ?, 'READY')",
    )
    .bind("bundle-lower")
    .bind("cn013")
    .bind("hash-lower")
    .bind("lower bundle")
    .execute(&pool)
    .await
    .expect("insert lower bundle");
    sqlx::query(
        "INSERT INTO bundles (id, issue_code, hash, name, status) VALUES (?, ?, ?, ?, 'READY')",
    )
    .bind("bundle-upper")
    .bind("CN013")
    .bind("hash-upper")
    .bind("upper bundle")
    .execute(&pool)
    .await
    .expect("insert upper bundle");

    db::prepare_schema(&pool, false)
        .await
        .expect("migrate issue codes");

    let issue_codes: Vec<String> = sqlx::query_scalar("SELECT code FROM issues ORDER BY code")
        .fetch_all(&pool)
        .await
        .expect("fetch issues");
    assert_eq!(issue_codes, vec!["CN013"]);

    let bundle_issue_codes: Vec<String> =
        sqlx::query_scalar("SELECT issue_code FROM bundles ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("fetch bundle issue codes");
    assert_eq!(bundle_issue_codes, vec!["CN013", "CN013"]);
}

#[actix_web::test]
async fn processing_bundles_cannot_be_deleted() {
    let test_dir = TestDir::new("rain-processing-delete");
    let db_url = sqlite_url(&test_dir.path.join("rain.db"));
    let data_root = test_dir.path.join("uploads");
    fs::create_dir_all(&data_root).expect("create data root");

    let pool = db::init_pool(&db_url).expect("init sqlite pool");
    db::prepare_schema(&pool, true)
        .await
        .expect("prepare schema");
    sqlx::query("INSERT INTO issues (code, name) VALUES (?, ?)")
        .bind("BUSY")
        .bind("BUSY")
        .execute(&pool)
        .await
        .expect("insert issue");
    sqlx::query(
        "INSERT INTO bundles (id, issue_code, hash, name, status) VALUES (?, ?, ?, ?, 'PROCESSING')",
    )
    .bind("busy-bundle")
    .bind("BUSY")
    .bind("busy-hash")
    .bind("busy bundle")
    .execute(&pool)
    .await
    .expect("insert processing bundle");
    let file_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO files (bundle_id, name, path, is_dir, status)
        VALUES (?, ?, ?, 0, 'PROCESSING')
        RETURNING id
        "#,
    )
    .bind("busy-bundle")
    .bind("busy.log")
    .bind("/busy-hash/busy.log")
    .fetch_one(&pool)
    .await
    .expect("insert processing file");
    let segment_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO log_segments (bundle_id, file_id, content, line_offset, line_end, chunk_index)
        VALUES (?, ?, ?, 0, 1, 0)
        RETURNING id
        "#,
    )
    .bind("busy-bundle")
    .bind(file_id)
    .bind("ERROR processing partial index")
    .fetch_one(&pool)
    .await
    .expect("insert processing segment");
    sqlx::query(
        r#"
        INSERT INTO log_segments_fts (content, segment_id, bundle_id, file_id, timeline)
        VALUES (?, ?, ?, ?, NULL)
        "#,
    )
    .bind("ERROR processing partial index")
    .bind(segment_id)
    .bind("busy-bundle")
    .bind(file_id)
    .execute(&pool)
    .await
    .expect("insert processing fts");

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppState { pool, data_root }))
            .configure(routes::register),
    )
    .await;

    let delete_bundle = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/issues/BUSY/bundles/busy-hash")
            .to_request(),
    )
    .await;
    assert_eq!(delete_bundle.status(), StatusCode::CONFLICT);

    let bundle_search = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/log/v2/busy-hash/search?q=processing")
            .to_request(),
    )
    .await;
    assert_eq!(bundle_search.status(), StatusCode::CONFLICT);

    let file_root = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/files/v1/busy-hash/files/root")
            .to_request(),
    )
    .await;
    assert_eq!(file_root.status(), StatusCode::CONFLICT);

    let issue_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/BUSY/search?q=processing&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(issue_search["total"], 0);

    let delete_issue = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/issues/BUSY")
            .to_request(),
    )
    .await;
    assert_eq!(delete_issue.status(), StatusCode::CONFLICT);
}

async fn wait_for_issue_ready(pool: &sqlx::SqlitePool, issue_code: &str) {
    wait_for_issue_status(pool, issue_code, "READY").await;
}

async fn wait_for_issue_status(pool: &sqlx::SqlitePool, issue_code: &str, status: &str) {
    for _ in 0..100 {
        let ready: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM bundles
                WHERE issue_code = ? AND status = ?
            )
            "#,
        )
        .bind(issue_code)
        .bind(status)
        .fetch_one(pool)
        .await
        .expect("poll issue status");
        if ready {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("issue {issue_code} did not become {status}");
}

fn multipart_body(boundary: &str, issue_code: &str, filename: &str, content: &str) -> Vec<u8> {
    multipart_body_bytes(
        boundary,
        issue_code,
        filename,
        "text/plain",
        content.as_bytes(),
    )
}

fn multipart_body_bytes(
    boundary: &str,
    issue_code: &str,
    filename: &str,
    content_type: &str,
    content: &[u8],
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\n\
Content-Disposition: form-data; name=\"issue_code\"\r\n\r\n\
{issue_code}\r\n\
--{boundary}\r\n\
Content-Disposition: form-data; name=\"files\"; filename=\"{filename}\"\r\n\
Content-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

fn multipart_body_multi(boundary: &str, issue_code: &str, files: &[(&str, &str)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\n\
Content-Disposition: form-data; name=\"issue_code\"\r\n\r\n\
{issue_code}\r\n"
        )
        .as_bytes(),
    );
    for (filename, content) in files {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\n\
Content-Disposition: form-data; name=\"files\"; filename=\"{filename}\"\r\n\
Content-Type: text/plain\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(content.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

fn gzip_bytes(content: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(content.as_bytes()).expect("write gzip");
    encoder.finish().expect("finish gzip")
}

fn tar_gz_bytes(path: &str, content: &str) -> Vec<u8> {
    tar_gz_multi(&[(path, content)])
}

fn tar_gz_multi(files: &[(&str, &str)]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut builder = tar::Builder::new(&mut encoder);
        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, *path, content.as_bytes())
                .expect("append tar entry");
        }
        builder.finish().expect("finish tar");
    }
    encoder.finish().expect("finish tar.gz")
}

fn sqlite_url(path: &Path) -> String {
    format!("sqlite://{}", path.display().to_string().replace('\\', "/"))
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&path).expect("create temp test dir");
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
