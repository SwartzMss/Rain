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

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppState { pool, data_root }))
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
    let upload: Value = test::call_and_read_body_json(
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
    let bundle_hash = upload
        .get("bundle_hash")
        .and_then(Value::as_str)
        .expect("bundle hash");
    assert_eq!(upload["issue_code"], "SMOKE");

    let search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/SMOKE/search?q=smoke&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(search["total"], 1);
    assert_eq!(search["hits"][0]["snippet"], "ERROR smoke works");

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

    let gz_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/GZIP/search?q=compressed&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(gz_search["total"], 1);
    assert_eq!(
        gz_search["hits"][0]["snippet"],
        "ERROR compressed smoke works"
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

    let tar_search: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/TARGZ/search?q=targz&size=10")
            .to_request(),
    )
    .await;
    assert_eq!(tar_search["total"], 1);
    assert_eq!(tar_search["hits"][0]["snippet"], "ERROR targz smoke works");

    let tree: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/files/v1/{bundle_hash}/files/root"))
            .to_request(),
    )
    .await;
    assert_eq!(tree["children"].as_array().expect("children").len(), 1);
    assert_eq!(tree["children"][0]["meta"]["kind"], "uploaded_file");

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

fn gzip_bytes(content: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(content.as_bytes()).expect("write gzip");
    encoder.finish().expect("finish gzip")
}

fn tar_gz_bytes(path: &str, content: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut builder = tar::Builder::new(&mut encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, path, content.as_bytes())
            .expect("append tar entry");
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
