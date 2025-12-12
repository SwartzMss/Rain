use actix_web::{HttpResponse, get, web};
use serde::Deserialize;
use serde_json::json;

use crate::{
    AppState,
    error::AppError,
    models::{
        files::{FileNode, FileNodeResponse},
        issues::{IssueBundlesResponse, UploadStatus, UploadStatusWrapper},
        logs::{LogSearchHit, LogSearchResponse},
    },
};

pub fn register(cfg: &mut web::ServiceConfig) {
    cfg.service(health).service(
        web::scope("/api")
            .service(get_issue_bundles)
            .service(get_file_node)
            .service(search_logs),
    );
}

#[get("/healthz")]
async fn health() -> HttpResponse {
    HttpResponse::Ok().json(json!({
        "status": "ok",
        "service": "rain-backend",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

#[get("/api/issues/{issue_id}")]
async fn get_issue_bundles(path: web::Path<String>) -> Result<HttpResponse, AppError> {
    let issue_id = path.into_inner();
    let bundles = IssueBundlesResponse {
        name: issue_id.clone(),
        log_bundles: vec![
            sample_bundle("qqmzk6", "0608.zip", UploadStatus::Ready),
            sample_bundle("lp1yp7", "0704.zip", UploadStatus::Processing),
        ],
    };
    Ok(HttpResponse::Ok().json(bundles))
}

#[derive(Deserialize)]
struct FilePath {
    bundle_id: String,
    file_id: String,
}

#[get("/api/files/v1/{bundle_id}/files/{file_id}")]
async fn get_file_node(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let is_root = file_id == "root";

    let node = FileNode {
        id: file_id.clone(),
        name: if is_root {
            format!("{bundle_id}_root")
        } else {
            format!("{file_id}.log")
        },
        path: if is_root {
            format!("/{bundle_id}")
        } else {
            format!("/{bundle_id}/{file_id}.log")
        },
        is_dir: is_root,
        size_bytes: Some(if is_root { 0 } else { 1_048_576 }),
        mime_type: if is_root {
            None
        } else {
            Some("text/plain".into())
        },
        status: Some("READY".into()),
        meta: Some(json!({
            "bundle": bundle_id,
            "storage_root": state.data_root.display().to_string()
        })),
    };

    let children = if is_root {
        vec![
            FileNode {
                id: "110".into(),
                name: "system.log".into(),
                path: format!("/{bundle_id}/system.log"),
                is_dir: false,
                size_bytes: Some(256_000),
                mime_type: Some("text/plain".into()),
                status: Some("READY".into()),
                meta: None,
            },
            FileNode {
                id: "210".into(),
                name: "runtime".into(),
                path: format!("/{bundle_id}/runtime"),
                is_dir: true,
                size_bytes: None,
                mime_type: None,
                status: Some("READY".into()),
                meta: None,
            },
        ]
    } else {
        vec![]
    };

    Ok(HttpResponse::Ok().json(FileNodeResponse { node, children }))
}

#[derive(Deserialize)]
struct LogQuery {
    q: String,
    timeline: Option<String>,
}

#[get("/api/log/v2/{bundle_id}/search")]
async fn search_logs(
    path: web::Path<String>,
    query: web::Query<LogQuery>,
) -> Result<HttpResponse, AppError> {
    let bundle_id = path.into_inner();
    let term = query.into_inner();

    let hits = vec![
        LogSearchHit {
            file_id: "110".into(),
            path: format!("/{bundle_id}/system.log"),
            snippet: format!("[{bundle_id}] ... {} ...", term.q),
            timeline: term.timeline.clone().or(Some("all".into())),
            offset: Some(1024),
        },
        LogSearchHit {
            file_id: "210-1".into(),
            path: format!("/{bundle_id}/runtime/pm.log"),
            snippet: "WARN pm startup took 5s".into(),
            timeline: term.timeline.clone(),
            offset: Some(2048),
        },
    ];

    Ok(HttpResponse::Ok().json(LogSearchResponse {
        total: hits.len() as u64,
        hits,
    }))
}

fn sample_bundle(
    hash: &str,
    name: &str,
    status: UploadStatus,
) -> crate::models::issues::UploadSummary {
    crate::models::issues::UploadSummary {
        hash: hash.into(),
        name: name.into(),
        status: UploadStatusWrapper {
            upload_status: status,
        },
    }
}
