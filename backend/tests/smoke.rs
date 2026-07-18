use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
};

use actix_web::{App, http::StatusCode, test, web};
use backend::{AppState, config::AppLimits, db, routes};
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
    let failure_reason_columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('bundles') WHERE name = 'failure_reason'",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect bundles schema");
    assert_eq!(failure_reason_columns, 1);
    insert_issues(
        &pool,
        &[
            "SMOKE",
            "GZIP",
            "TARGZ",
            "NESTEDZIP",
            "NESTEDCHAIN",
            "DEPTHFAIL",
            "BINARY",
            "DIRDELETE",
            "FAILEDCASE",
            "LARGE",
            "BADUTF8",
            "LONGLINE",
            "COLLISION",
        ],
    )
    .await;
    let app_pool = pool.clone();
    let mut limits = AppLimits::default();
    limits.indexing.max_indexed_line_size = 64;
    limits.api.max_preview_line_size = 256;

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppState::new(
                app_pool,
                data_root.clone(),
                limits,
            )))
            .configure(routes::register),
    )
    .await;

    let boundary = format!("rain-{}", Uuid::new_v4().simple());
    let upload_body = multipart_body(
        &boundary,
        "SMOKE",
        "app.log",
        "INFO boot\nERROR smoke works requestId=abcdef123456 中文连续文本\n",
    );
    let upload_response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/SMOKE/uploads")
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
    assert_eq!(upload["stage"], "RECEIVING");
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
    let direct_content_size: i64 = sqlx::query_scalar(
        "SELECT content_size_bytes FROM bundles WHERE issue_code = 'SMOKE' AND status = 'READY'",
    )
    .fetch_one(&pool)
    .await
    .expect("load direct content size");
    assert_eq!(
        direct_content_size,
        "INFO boot\nERROR smoke works requestId=abcdef123456 中文连续文本\n".len() as i64
    );
    let completed_task: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/uploads/{bundle_hash}"))
            .to_request(),
    )
    .await;
    assert_eq!(completed_task["stage"], "READY");
    assert!(completed_task["failure_reason"].is_null());

    let filename_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/SMOKE/search?q=app.log&mode=filename&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(filename_search["total"], 1);
    assert_eq!(filename_search["hits"][0]["path"], "app.log");
    assert_eq!(filename_search["hits"][0]["line_number"], Value::Null);

    let search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/SMOKE/search?q=smoke&mode=content&size=10")
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

    let substring_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/SMOKE/search?q=def123&mode=content&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(substring_search["total"], 1);
    assert!(
        substring_search["hits"][0]["snippet"]
            .as_str()
            .expect("substring snippet")
            .contains("abcdef123456")
    );

    let short_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/SMOKE/search?q=ER&mode=content&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(short_search["total"], 1);

    let gz_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let gz_content = "INFO gzip\nERROR compressed smoke works\n";
    let gz_bytes = gzip_bytes(gz_content);
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
            .uri("/api/issues/GZIP/uploads")
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
    let gzip_content_size: i64 = sqlx::query_scalar(
        "SELECT content_size_bytes FROM bundles WHERE issue_code = 'GZIP' AND status = 'READY'",
    )
    .fetch_one(&pool)
    .await
    .expect("load gzip content size");
    assert_eq!(gzip_content_size, gz_content.len() as i64);

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
            .uri("/api/issues/TARGZ/uploads")
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

    let nested_zip_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let inner_zip = zip_bytes(&[(
        "inner.log",
        b"INFO nested zip\nERROR nested zip search works\n",
    )]);
    let outer_zip = zip_bytes(&[("inner.zip", inner_zip.as_slice())]);
    let nested_zip_body = multipart_body_bytes(
        &nested_zip_boundary,
        "NESTEDZIP",
        "outer.zip",
        "application/zip",
        &outer_zip,
    );
    let nested_zip_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/NESTEDZIP/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={nested_zip_boundary}"),
            ))
            .set_payload(nested_zip_body)
            .to_request(),
    )
    .await;
    assert_eq!(nested_zip_upload["issue_code"], "NESTEDZIP");
    wait_for_issue_ready(&pool, "NESTEDZIP").await;
    let nested_zip_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/NESTEDZIP/search?q=nested%20zip%20search&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(nested_zip_search["total"], 1);
    assert!(
        nested_zip_search["hits"][0]["path"]
            .as_str()
            .expect("nested zip path")
            .contains("inner.log")
    );

    let nested_chain_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let nested_gzip = gzip_bytes("INFO deep chain\nERROR nested chain search works\n");
    let nested_chain_zip = zip_bytes(&[("deep.log.gz", nested_gzip.as_slice())]);
    let nested_chain_tar = tar_gz_multi_bytes(&[("middle.zip", nested_chain_zip.as_slice())]);
    let nested_chain_body = multipart_body_bytes(
        &nested_chain_boundary,
        "NESTEDCHAIN",
        "outer.tar.gz",
        "application/gzip",
        &nested_chain_tar,
    );
    let nested_chain_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/NESTEDCHAIN/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={nested_chain_boundary}"),
            ))
            .set_payload(nested_chain_body)
            .to_request(),
    )
    .await;
    assert_eq!(nested_chain_upload["issue_code"], "NESTEDCHAIN");
    wait_for_issue_ready(&pool, "NESTEDCHAIN").await;
    let nested_chain_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/NESTEDCHAIN/search?q=nested%20chain%20search&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(nested_chain_search["total"], 1);
    assert!(
        nested_chain_search["hits"][0]["path"]
            .as_str()
            .expect("nested chain path")
            .contains("deep.log")
    );

    let depth_fail_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let mut depth_fail_bytes = zip_bytes(&[("too-deep.log", b"ERROR must not be indexed\n")]);
    for depth in 0..16 {
        let filename = format!("level-{depth}.zip");
        depth_fail_bytes = zip_bytes(&[(&filename, depth_fail_bytes.as_slice())]);
    }
    let depth_fail_body = multipart_body_bytes(
        &depth_fail_boundary,
        "DEPTHFAIL",
        "depth-fail.zip",
        "application/zip",
        &depth_fail_bytes,
    );
    let depth_fail_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/DEPTHFAIL/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={depth_fail_boundary}"),
            ))
            .set_payload(depth_fail_body)
            .to_request(),
    )
    .await;
    let depth_fail_bundle = depth_fail_upload["bundle_hash"]
        .as_str()
        .expect("depth failure bundle");
    wait_for_issue_status(&pool, "DEPTHFAIL", "FAILED").await;
    let depth_fail_tree = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/files/v1/{depth_fail_bundle}/files/root"))
            .to_request(),
    )
    .await;
    assert_eq!(depth_fail_tree.status(), StatusCode::CONFLICT);

    let binary_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let executable_bytes = [b'M', b'Z', 0, 1, 2, 3, 255];
    let unknown_text = b"INFO probe\nERROR unknown extension text works\n";
    let unknown_binary = [0, 159, 146, 150, 1, 2, 3];
    let docx_bytes = zip_bytes(&[(
        "word/document.xml",
        b"<document>must remain binary</document>",
    )]);
    let binary_body = multipart_body_multi_bytes(
        &binary_boundary,
        "BINARY",
        &[
            ("tool.exe", "application/octet-stream", &executable_bytes),
            (
                "report.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                docx_bytes.as_slice(),
            ),
            ("notes.data", "application/octet-stream", unknown_text),
            ("blob.data", "application/octet-stream", &unknown_binary),
        ],
    );
    let binary_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/BINARY/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={binary_boundary}"),
            ))
            .set_payload(binary_body)
            .to_request(),
    )
    .await;
    let binary_bundle = binary_upload["bundle_hash"]
        .as_str()
        .expect("binary bundle");
    wait_for_issue_ready(&pool, "BINARY").await;
    let binary_tree: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/files/v1/{binary_bundle}/files/root"))
            .to_request(),
    )
    .await;
    let binary_children = binary_tree["children"]
        .as_array()
        .expect("binary tree children");
    let executable_node = binary_children
        .iter()
        .find(|node| node["name"] == "tool.exe")
        .expect("executable node");
    let docx_node = binary_children
        .iter()
        .find(|node| node["name"] == "report.docx")
        .expect("docx node");
    let text_probe_node = binary_children
        .iter()
        .find(|node| node["name"] == "notes.data")
        .expect("unknown text node");
    let binary_probe_node = binary_children
        .iter()
        .find(|node| node["name"] == "blob.data")
        .expect("unknown binary node");
    assert_eq!(executable_node["preview_kind"], "binary");
    assert_eq!(docx_node["preview_kind"], "binary");
    assert_eq!(text_probe_node["preview_kind"], "text");
    assert_eq!(binary_probe_node["preview_kind"], "binary");

    for node in [executable_node, docx_node, binary_probe_node] {
        let file_id = node["id"].as_str().expect("binary file id");
        let content_response = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(&format!(
                    "/api/files/v1/{binary_bundle}/files/{file_id}/content"
                ))
                .to_request(),
        )
        .await;
        assert_eq!(content_response.status(), StatusCode::BAD_REQUEST);
        let content_error: Value = test::read_body_json(content_response).await;
        assert!(
            content_error["error"]
                .as_str()
                .expect("binary content error")
                .contains("text preview is not supported")
        );
        let lines_response = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(&format!(
                    "/api/files/v1/{binary_bundle}/files/{file_id}/lines"
                ))
                .to_request(),
        )
        .await;
        assert_eq!(lines_response.status(), StatusCode::BAD_REQUEST);
        let lines_error: Value = test::read_body_json(lines_response).await;
        assert!(
            lines_error["error"]
                .as_str()
                .expect("binary lines error")
                .contains("text preview is not supported")
        );
    }

    let executable_id = executable_node["id"].as_str().expect("executable id");
    let download_response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/files/v1/{binary_bundle}/files/{executable_id}/download"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(download_response.status(), StatusCode::OK);
    assert!(
        download_response
            .headers()
            .get("content-disposition")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("attachment"))
    );

    let docx_id = docx_node["id"].as_str().expect("docx id");
    let docx_detail: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/files/v1/{binary_bundle}/files/{docx_id}"))
            .to_request(),
    )
    .await;
    assert!(
        docx_detail["children"]
            .as_array()
            .expect("docx children")
            .is_empty()
    );

    let unknown_text_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/BINARY/search?q=unknown%20extension%20text&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(unknown_text_search["total"], 1);
    let binary_file_ids = [
        executable_id.parse::<i64>().expect("numeric executable id"),
        docx_id.parse::<i64>().expect("numeric docx id"),
        binary_probe_node["id"]
            .as_str()
            .expect("binary probe id")
            .parse::<i64>()
            .expect("numeric binary probe id"),
    ];
    for file_id in binary_file_ids {
        let artifacts = sqlx::query_as::<_, (i64, i64, i64)>(
            r#"
            SELECT
                (SELECT COUNT(*) FROM log_line_offsets WHERE file_id = ?),
                (SELECT COUNT(*) FROM log_segments WHERE file_id = ?),
                (SELECT COUNT(*) FROM log_segments_fts fts
                 JOIN log_segments ls ON ls.id = fts.rowid WHERE ls.file_id = ?)
            "#,
        )
        .bind(file_id)
        .bind(file_id)
        .bind(file_id)
        .fetch_one(&pool)
        .await
        .expect("count binary index artifacts");
        assert_eq!(artifacts, (0, 0, 0));
    }

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
            .uri("/api/issues/DIRDELETE/uploads")
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
    let target_dir = dirs
        .iter()
        .find(|node| node["name"] == "a_b")
        .expect("a_b dir");
    let target_dir_id = target_dir["id"].as_str().expect("a_b dir id");
    let blobs_before_directory_delete: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blobs")
        .fetch_one(&pool)
        .await
        .expect("count blobs before directory deletion");
    let target_meta: String = sqlx::query_scalar("SELECT meta FROM files WHERE id = ?")
        .bind(target_dir_id.parse::<i64>().expect("numeric file id"))
        .fetch_one(&pool)
        .await
        .expect("load target metadata");
    assert!(!target_meta.contains("storage_path"));
    assert!(!data_root.join(&delete_dir_bundle).exists());
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
    let blobs_after_directory_delete: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blobs")
        .fetch_one(&pool)
        .await
        .expect("count blobs after directory deletion");
    assert_eq!(blobs_after_directory_delete, blobs_before_directory_delete);
    let gc_store = backend::blob_store::LocalCasBlobStore::new(data_root.clone());
    assert_eq!(
        backend::blob_store::garbage_collect_unreferenced_blobs(&pool, &gc_store)
            .await
            .expect("scan unreferenced blobs within grace period"),
        0
    );
    let grace_candidates: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM blobs WHERE unreferenced_at IS NOT NULL AND state = 'READY'",
    )
    .fetch_one(&pool)
    .await
    .expect("count blobs in GC grace period");
    assert!(grace_candidates > 0);

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
            .uri("/api/issues/FAILEDCASE/uploads")
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
    assert!(failed_task["failure_reason"].is_string());
    assert!(failed_task["failure_stage"].is_string());
    assert!(failed_task["failure_code"].is_string());
    assert!(failed_task["retryable"].is_boolean());
    let failed_issue: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/FAILEDCASE")
            .to_request(),
    )
    .await;
    assert_eq!(
        failed_issue["log_bundles"][0]["failure_reason"],
        failed_task["failure_reason"]
    );
    let failed_tree = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/files/v1/{failed_bundle}/files/root"))
            .to_request(),
    )
    .await;
    assert_eq!(failed_tree.status(), StatusCode::CONFLICT);

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

    let exact_file_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/log/v2/{bundle_hash}/search?q=smoke&file_id={app_file_id}&size=10"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(exact_file_search["total"], 1);

    let other_file_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/log/v2/{bundle_hash}/search?q=smoke&file_id=999999999&size=10"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(other_file_search["total"], 0);

    let temporary_preview: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/temp-results/preview")
            .set_json(serde_json::json!({
                "expression": "ERROR AND NOT timeout",
                "bundle_hash": bundle_hash,
                "file_id": app_file_id,
                "from": 0,
                "size": 50
            }))
            .to_request(),
    )
    .await;
    assert_eq!(temporary_preview["total"], 1);
    let preview_result_id = temporary_preview["result_id"]
        .as_str()
        .expect("preview result id");
    assert_eq!(temporary_preview["lines"][0]["line_number"], 1);
    assert_eq!(
        temporary_preview["lines"][0]["content"],
        "ERROR smoke works requestId=abcdef123456 中文连续文本"
    );
    let preview_lines: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/temp-results/{preview_result_id}/lines?start=0&limit=1000"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(preview_lines["lines"][0]["bundle_hash"], bundle_hash);
    assert_eq!(
        preview_lines["lines"][0]["file_id"],
        app_file_id.to_string()
    );
    assert_eq!(preview_lines["lines"][0]["line_number"], 1);
    let preview_base = data_root
        .join("temp-results")
        .join(format!("{preview_result_id}.log"));
    assert!(preview_base.exists());
    assert!(preview_base.with_extension("meta").exists());
    assert!(preview_base.with_extension("idx").exists());
    let delete_preview = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/temp-results/{preview_result_id}"))
            .to_request(),
    )
    .await;
    assert_eq!(delete_preview.status(), StatusCode::NO_CONTENT);
    assert!(!preview_base.exists());
    assert!(!preview_base.with_extension("meta").exists());
    assert!(!preview_base.with_extension("idx").exists());

    let literal_phrase_preview: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/temp-results/preview")
            .set_json(serde_json::json!({
                "expression": "\"ERROR smoke works\"",
                "bundle_hash": bundle_hash,
                "file_id": app_file_id,
                "from": 0,
                "size": 50
            }))
            .to_request(),
    )
    .await;
    assert_eq!(literal_phrase_preview["total"], 1);

    let invalid_expression_response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/temp-results/preview")
            .set_json(serde_json::json!({
                "expression": "\"ERROR\" AND",
                "bundle_hash": bundle_hash,
                "file_id": app_file_id
            }))
            .to_request(),
    )
    .await;
    assert_eq!(
        invalid_expression_response.status(),
        StatusCode::BAD_REQUEST
    );
    let invalid_expression_body: Value = test::read_body_json(invalid_expression_response).await;
    let invalid_expression_message = invalid_expression_body["error"]
        .as_str()
        .expect("invalid expression error");
    assert!(invalid_expression_message.contains("搜索条件无效"));
    assert!(invalid_expression_message.contains("位置"));

    let temporary_result_response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/temp-results")
            .set_json(serde_json::json!({
                "expression": "ERROR AND NOT timeout",
                "bundle_hash": bundle_hash,
                "file_id": app_file_id
            }))
            .to_request(),
    )
    .await;
    assert_eq!(temporary_result_response.status(), StatusCode::CREATED);
    let temporary_result: Value = test::read_body_json(temporary_result_response).await;
    let temporary_result_id = temporary_result["id"]
        .as_str()
        .expect("temporary result id");
    assert_eq!(temporary_result["line_count"], 1);

    let temporary_lines: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/temp-results/{temporary_result_id}/lines?start=0&limit=1000"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(
        temporary_lines["lines"][0]["content"],
        "ERROR smoke works requestId=abcdef123456 中文连续文本"
    );

    let temporary_download = test::call_and_read_body(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/temp-results/{temporary_result_id}/download"))
            .to_request(),
    )
    .await;
    assert_eq!(
        temporary_download,
        "ERROR smoke works requestId=abcdef123456 中文连续文本\n"
    );

    let delete_temporary_result = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/temp-results/{temporary_result_id}"))
            .to_request(),
    )
    .await;
    assert_eq!(delete_temporary_result.status(), StatusCode::NO_CONTENT);

    let missing_temporary_result = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/temp-results/{temporary_result_id}"))
            .to_request(),
    )
    .await;
    assert_eq!(missing_temporary_result.status(), StatusCode::NOT_FOUND);

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
    assert_eq!(
        lines["lines"][0]["content"],
        "ERROR smoke works requestId=abcdef123456 中文连续文本"
    );

    let download = test::call_and_read_body(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/files/v1/{bundle_hash}/files/{app_file_id}/download"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(
        download,
        "INFO boot\nERROR smoke works requestId=abcdef123456 中文连续文本\n"
    );

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
            .uri("/api/issues/LARGE/uploads")
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

    let large_tree: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/files/v1/{}/files/root",
                large_upload["bundle_hash"]
                    .as_str()
                    .expect("large bundle hash")
            ))
            .to_request(),
    )
    .await;
    let large_file_id = large_tree["children"][0]["id"]
        .as_str()
        .expect("large file id");
    let large_lines: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/files/v1/{}/files/{large_file_id}/lines?start=0&limit=3000",
                large_upload["bundle_hash"]
                    .as_str()
                    .expect("large bundle hash")
            ))
            .to_request(),
    )
    .await;
    assert_eq!(large_lines["limit"], 3000);
    assert_eq!(
        large_lines["lines"].as_array().expect("large lines").len(),
        2501
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
            .uri("/api/issues/BADUTF8/uploads")
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
    let mut long_line = b"INDEX_PREFIX ".to_vec();
    long_line.extend(std::iter::repeat_n(b'a', 80));
    long_line.extend_from_slice(b" INDEX_SUFFIX ");
    long_line.extend(std::iter::repeat_n(b'b', 240));
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
            .uri("/api/issues/LONGLINE/uploads")
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
    let indexed_prefix: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/LONGLINE/search?q=INDEX_PREFIX&size=10")
            .to_request(),
    )
    .await;
    let omitted_suffix: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/LONGLINE/search?q=INDEX_SUFFIX&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(indexed_prefix["total"], 1);
    assert_eq!(indexed_prefix["hits"][0]["bundle_hash"], long_line_bundle);
    assert_eq!(indexed_prefix["hits"][0]["file_id"], long_line_file);
    assert_eq!(indexed_prefix["hits"][0]["line_number"], 0);
    assert_eq!(omitted_suffix["total"], 0);
    assert_eq!(long_line_lines["lines"][0]["truncated"], true);
    let previewed_long_line = long_line_lines["lines"][0]["content"]
        .as_str()
        .expect("long line content");
    assert!(previewed_long_line.len() > 64);
    assert!(previewed_long_line.len() <= 256 + " ... [line truncated]".len());
    assert!(previewed_long_line.ends_with("[line truncated]"));
    let following_lines: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/files/v1/{long_line_bundle}/files/{long_line_file}/lines?start=1&limit=1"
            ))
            .to_request(),
    )
    .await;
    assert_eq!(following_lines["lines"][0]["line_number"], 1);
    assert_eq!(
        following_lines["lines"][0]["content"],
        "ERROR after long line"
    );
    assert_eq!(following_lines["lines"][0]["truncated"], false);

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
            .uri("/api/issues/COLLISION/uploads")
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
async fn issue_quota_overflow_fails_and_releases_bundle_content() {
    let test_dir = TestDir::new("rain-quota-overflow");
    let db_url = sqlite_url(&test_dir.path.join("rain.db"));
    let data_root = test_dir.path.join("uploads");
    fs::create_dir_all(&data_root).expect("create data root");
    let pool = db::init_pool(&db_url).expect("init sqlite pool");
    db::prepare_schema(&pool, true)
        .await
        .expect("prepare schema");
    insert_issues(&pool, &["QUOTAFAIL"]).await;
    let mut limits = AppLimits::default();
    limits.issue_max_content_size = 16;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppState::new(
                pool.clone(),
                data_root,
                limits,
            )))
            .configure(routes::register),
    )
    .await;

    let boundary = format!("rain-{}", Uuid::new_v4().simple());
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/QUOTAFAIL/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            ))
            .set_payload(multipart_body(
                &boundary,
                "QUOTAFAIL",
                "too-large.log",
                "12345678901234567",
            ))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    wait_for_issue_status(&pool, "QUOTAFAIL", "FAILED").await;

    let (content_size, failure_reason): (i64, Option<String>) = sqlx::query_as(
        "SELECT content_size_bytes, failure_reason FROM bundles WHERE issue_code = 'QUOTAFAIL'",
    )
    .fetch_one(&pool)
    .await
    .expect("load failed bundle");
    let file_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE bundle_id IN (SELECT id FROM bundles WHERE issue_code = 'QUOTAFAIL')",
    )
    .fetch_one(&pool)
    .await
    .expect("count failed files");
    assert_eq!(content_size, 0);
    assert_eq!(file_count, 0);
    assert!(
        failure_reason
            .expect("quota failure reason")
            .contains("16 B")
    );

    let exact_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let exact_upload: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/QUOTAFAIL/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={exact_boundary}"),
            ))
            .set_payload(multipart_body(
                &exact_boundary,
                "QUOTAFAIL",
                "exact.log",
                "1234567890123456",
            ))
            .to_request(),
    )
    .await;
    wait_for_issue_ready(&pool, "QUOTAFAIL").await;
    let exact_hash = exact_upload["bundle_hash"]
        .as_str()
        .expect("exact bundle hash");
    let ready_size: i64 = sqlx::query_scalar(
        "SELECT content_size_bytes FROM bundles WHERE issue_code = 'QUOTAFAIL' AND status = 'READY'",
    )
    .fetch_one(&pool)
    .await
    .expect("load exact bundle size");
    assert_eq!(ready_size, 16);

    let delete = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/issues/QUOTAFAIL/bundles/{exact_hash}"))
            .to_request(),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);
    let deleted_bundle: (String, Option<String>) =
        sqlx::query_as("SELECT status, deleted_at FROM bundles WHERE hash = ?")
            .bind(exact_hash)
            .fetch_one(&pool)
            .await
            .expect("load logically deleted bundle");
    assert_eq!(deleted_bundle.0, "DELETED");
    assert!(deleted_bundle.1.is_some());
    let deleted_bundle_files: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE bundle_id = (SELECT id FROM bundles WHERE hash = ?)",
    )
    .bind(exact_hash)
    .fetch_one(&pool)
    .await
    .expect("count deleted bundle file references");
    assert_eq!(deleted_bundle_files, 0);

    let replacement_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let replacement = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/QUOTAFAIL/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={replacement_boundary}"),
            ))
            .set_payload(multipart_body(
                &replacement_boundary,
                "QUOTAFAIL",
                "replacement.log",
                "abcdefghijklmnop",
            ))
            .to_request(),
    )
    .await;
    assert_eq!(replacement.status(), StatusCode::ACCEPTED);
    wait_for_issue_ready(&pool, "QUOTAFAIL").await;
    let replacement_size: i64 = sqlx::query_scalar(
        "SELECT content_size_bytes FROM bundles WHERE issue_code = 'QUOTAFAIL' AND status = 'READY'",
    )
    .fetch_one(&pool)
    .await
    .expect("load replacement size");
    assert_eq!(replacement_size, 16);
}

#[actix_web::test]
async fn issue_creation_and_upload_require_existing_issue() {
    let test_dir = TestDir::new("rain-issue-create");
    let db_url = sqlite_url(&test_dir.path.join("rain.db"));
    let data_root = test_dir.path.join("uploads");
    fs::create_dir_all(&data_root).expect("create data root");

    let pool = db::init_pool(&db_url).expect("init sqlite pool");
    db::prepare_schema(&pool, true)
        .await
        .expect("prepare schema");
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppState::new(
                pool.clone(),
                data_root.clone(),
                AppLimits::default(),
            )))
            .configure(routes::register),
    )
    .await;

    let create = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues")
            .set_json(serde_json::json!({
                "code": "new001",
                "name": "First issue"
            }))
            .to_request(),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let created: Value = test::read_body_json(create).await;
    assert_eq!(created["code"], "NEW001");
    assert_eq!(created["name"], "First issue");
    assert_eq!(created["bundle_count"], 0);

    let duplicate = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues")
            .set_json(serde_json::json!({ "code": "NEW001" }))
            .to_request(),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let invalid = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues")
            .set_json(serde_json::json!({ "code": "BAD/ID" }))
            .to_request(),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let unicode_name = "中".repeat(128);
    let unicode = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues")
            .set_json(serde_json::json!({
                "code": "unicode",
                "name": unicode_name
            }))
            .to_request(),
    )
    .await;
    assert_eq!(unicode.status(), StatusCode::CREATED);

    let issues: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get().uri("/api/issues").to_request(),
    )
    .await;
    let created_issue = issues
        .as_array()
        .expect("issues")
        .iter()
        .find(|issue| issue["code"] == "NEW001")
        .expect("created issue in list");
    assert_eq!(created_issue["bundle_count"], 0);

    let missing_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let missing_upload_body = multipart_body(
        &missing_boundary,
        "MISSING",
        "missing.log",
        "ERROR missing\n",
    );
    let missing_upload = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/MISSING/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={missing_boundary}"),
            ))
            .set_payload(missing_upload_body)
            .to_request(),
    )
    .await;
    assert_eq!(missing_upload.status(), StatusCode::NOT_FOUND);
    let temp_root = data_root.join(".tmp");
    assert!(
        !temp_root.exists()
            || fs::read_dir(&temp_root)
                .expect("read temp root")
                .next()
                .is_none()
    );

    let upload_boundary = format!("rain-{}", Uuid::new_v4().simple());
    let upload_body = multipart_body(
        &upload_boundary,
        "NEW001",
        "app.log",
        "ERROR created upload\n",
    );
    let upload = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/NEW001/uploads")
            .insert_header((
                "content-type",
                format!("multipart/form-data; boundary={upload_boundary}"),
            ))
            .set_payload(upload_body)
            .to_request(),
    )
    .await;
    assert_eq!(upload.status(), StatusCode::ACCEPTED);
    wait_for_issue_ready(&pool, "NEW001").await;

    sqlx::query("UPDATE bundles SET created_at = datetime('now', '-2 days') WHERE issue_code = ?")
        .bind("NEW001")
        .execute(&pool)
        .await
        .expect("age bundle");
    let removed = db::cleanup_expired_bundles(&pool, 1)
        .await
        .expect("cleanup expired bundles");
    assert_eq!(removed, 1);
    let still_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM issues WHERE code = ?)")
            .bind("NEW001")
            .fetch_one(&pool)
            .await
            .expect("issue exists after cleanup");
    assert!(still_exists);

    let delete_empty = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/issues/NEW001")
            .to_request(),
    )
    .await;
    assert_eq!(delete_empty.status(), StatusCode::NO_CONTENT);
}

#[actix_web::test]
async fn bundle_content_cleanup_runs_in_batches_and_preserves_bundle() {
    let test_dir = TestDir::new("rain-batched-cleanup");
    let db_url = sqlite_url(&test_dir.path.join("rain.db"));
    let pool = db::init_pool(&db_url).expect("init sqlite pool");
    db::prepare_schema(&pool, true)
        .await
        .expect("prepare schema");
    sqlx::query("INSERT INTO issues (code, name) VALUES ('CLEAN', 'CLEAN')")
        .execute(&pool)
        .await
        .expect("insert issue");
    sqlx::query(
        "INSERT INTO bundles (id, issue_code, hash, name, status) VALUES ('cleanup', 'CLEAN', 'cleanup-hash', 'cleanup', 'FAILED')",
    )
    .execute(&pool)
    .await
    .expect("insert bundle");

    for index in 0..3i64 {
        let file_id: i64 = sqlx::query_scalar(
            "INSERT INTO files (bundle_id, name, path, is_dir) VALUES ('cleanup', ?, ?, 0) RETURNING id",
        )
        .bind(format!("{index}.log"))
        .bind(format!("/{index}.log"))
        .fetch_one(&pool)
        .await
        .expect("insert file");
        sqlx::query(
            "INSERT INTO log_line_offsets (file_id, line_number, byte_offset) VALUES (?, 0, 0)",
        )
        .bind(file_id)
        .execute(&pool)
        .await
        .expect("insert offset");
        let _segment_id: i64 = sqlx::query_scalar(
            "INSERT INTO log_segments (bundle_id, file_id, content) VALUES ('cleanup', ?, ?) RETURNING id",
        )
        .bind(file_id)
        .bind(format!("ERROR cleanup {index}"))
        .fetch_one(&pool)
        .await
        .expect("insert segment");
    }

    let stats = db::cleanup_bundle_content_batched(&pool, "cleanup", 2)
        .await
        .expect("cleanup bundle content");
    assert_eq!(stats.files.rows, 3);

    for table in [
        "log_line_offsets",
        "log_segments_fts",
        "log_segments",
        "files",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .expect("count cleaned rows");
        assert_eq!(count, 0, "{table} should be empty");
    }
    let bundle_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM bundles WHERE id = 'cleanup')")
            .fetch_one(&pool)
            .await
            .expect("check retained bundle");
    assert!(bundle_exists);
}

#[actix_web::test]
async fn startup_recovery_marks_processing_bundle_failed_with_reason() {
    let test_dir = TestDir::new("rain-stale-recovery");
    let db_url = sqlite_url(&test_dir.path.join("rain.db"));
    let pool = db::init_pool(&db_url).expect("init sqlite pool");
    db::prepare_schema(&pool, true)
        .await
        .expect("prepare schema");
    sqlx::query("INSERT INTO issues (code, name) VALUES ('STALE', 'STALE')")
        .execute(&pool)
        .await
        .expect("insert issue");
    sqlx::query(
        "INSERT INTO bundles (id, issue_code, hash, name, status) VALUES ('stale', 'STALE', 'stale-hash', 'stale', 'PROCESSING')",
    )
    .execute(&pool)
    .await
    .expect("insert stale bundle");

    assert_eq!(
        db::fail_stale_processing_bundles(&pool)
            .await
            .expect("recover stale bundle"),
        1
    );
    let recovered: (String, String, Option<String>) = sqlx::query_as(
        "SELECT status, process_stage, failure_reason FROM bundles WHERE id = 'stale'",
    )
    .fetch_one(&pool)
    .await
    .expect("read recovered bundle");
    assert_eq!(recovered.0, "FAILED");
    assert_eq!(recovered.1, "FAILED");
    assert!(recovered.2.is_some_and(|reason| reason.contains("重启")));
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
    let _segment_id: i64 = sqlx::query_scalar(
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

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppState::new(
                pool,
                data_root,
                AppLimits::default(),
            )))
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
    let states: Vec<(String, String)> =
        sqlx::query_as("SELECT status, process_stage FROM bundles WHERE issue_code = ?")
            .bind(issue_code)
            .fetch_all(pool)
            .await
            .expect("inspect timed out issue status");
    panic!("issue {issue_code} did not become {status}; observed {states:?}");
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

async fn insert_issues(pool: &sqlx::SqlitePool, issue_codes: &[&str]) {
    for code in issue_codes {
        sqlx::query("INSERT INTO issues (code, name) VALUES (?, ?)")
            .bind(code)
            .bind(code)
            .execute(pool)
            .await
            .expect("insert issue");
    }
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

fn multipart_body_multi_bytes(
    boundary: &str,
    issue_code: &str,
    files: &[(&str, &str, &[u8])],
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\n\
Content-Disposition: form-data; name=\"issue_code\"\r\n\r\n\
{issue_code}\r\n"
        )
        .as_bytes(),
    );
    for (filename, content_type, content) in files {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\n\
Content-Disposition: form-data; name=\"files\"; filename=\"{filename}\"\r\n\
Content-Type: {content_type}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(content);
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
    let binary_files = files
        .iter()
        .map(|(path, content)| (*path, content.as_bytes()))
        .collect::<Vec<_>>();
    tar_gz_multi_bytes(&binary_files)
}

fn tar_gz_multi_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut builder = tar::Builder::new(&mut encoder);
        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, *path, *content)
                .expect("append tar entry");
        }
        builder.finish().expect("finish tar");
    }
    encoder.finish().expect("finish tar.gz")
}

fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (path, content) in files {
        writer.start_file(*path, options).expect("start zip entry");
        writer.write_all(content).expect("write zip entry");
    }
    writer.finish().expect("finish zip").into_inner()
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
