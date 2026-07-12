use actix_web::{
    HttpRequest, HttpResponse,
    http::header::{CACHE_CONTROL, CONTENT_TYPE},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../frontend/dist/"]
struct FrontendAssets;

fn asset_response(path: &str) -> Option<HttpResponse> {
    let asset = FrontendAssets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache_control = if path == "index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };

    Some(
        HttpResponse::Ok()
            .insert_header((CONTENT_TYPE, mime.as_ref()))
            .insert_header((CACHE_CONTROL, cache_control))
            .body(asset.data.into_owned()),
    )
}

pub async fn serve_frontend(req: HttpRequest) -> HttpResponse {
    let request_path = req.path().trim_start_matches('/');

    if request_path == "api" || request_path.starts_with("api/") {
        return HttpResponse::NotFound()
            .content_type("application/json")
            .body(r#"{"error":"api endpoint not found"}"#);
    }

    let asset_path = if request_path.is_empty() {
        "index.html"
    } else {
        request_path
    };

    if let Some(response) = asset_response(asset_path) {
        return response;
    }

    asset_response("index.html").unwrap_or_else(|| {
        HttpResponse::InternalServerError()
            .content_type("text/plain; charset=utf-8")
            .body("embedded frontend index.html is missing")
    })
}
