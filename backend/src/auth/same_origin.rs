use actix_web::{
    Error, HttpResponse,
    body::{EitherBody, MessageBody},
    dev::{ServiceRequest, ServiceResponse},
    http::{Method, header},
    middleware::Next,
};

pub async fn enforce_same_origin(
    request: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<EitherBody<impl MessageBody>>, Error> {
    if is_safe_method(request.method()) || has_same_origin(&request) {
        return Ok(next.call(request).await?.map_into_left_body());
    }

    Ok(request.into_response(
        HttpResponse::Forbidden()
            .json(serde_json::json!({
                "code": "CROSS_ORIGIN_REQUEST_REJECTED",
                "message": "不允许跨来源修改 Rain 数据"
            }))
            .map_into_right_body(),
    ))
}

fn is_safe_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

fn has_same_origin(request: &ServiceRequest) -> bool {
    if let Some(fetch_site) = request
        .headers()
        .get("Sec-Fetch-Site")
        .and_then(|value| value.to_str().ok())
    {
        return fetch_site.eq_ignore_ascii_case("same-origin");
    }

    let Some(origin) = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return true;
    };
    let connection = request.connection_info();
    origin.eq_ignore_ascii_case(&format!("{}://{}", connection.scheme(), connection.host()))
}
