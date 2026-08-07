use actix_web::{
    Error, HttpResponse,
    body::{EitherBody, MessageBody},
    dev::{ServiceRequest, ServiceResponse},
    http::{Method, header},
    middleware::Next,
};

const RAIN_BROWSER_HEADER: &str = "x-rain-browser";
const RAIN_BROWSER_HEADER_VALUE: &str = "1";

pub async fn enforce_same_origin(
    request: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<EitherBody<impl MessageBody>>, Error> {
    if is_safe_method(request.method())
        || has_same_origin(&request)
        || is_rain_browser_extension_request(&request)
    {
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

fn is_rain_browser_extension_request(request: &ServiceRequest) -> bool {
    let marked = request
        .headers()
        .get(RAIN_BROWSER_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == RAIN_BROWSER_HEADER_VALUE);
    if !marked {
        return false;
    }

    let fetch_site = request
        .headers()
        .get("Sec-Fetch-Site")
        .and_then(|value| value.to_str().ok());
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());

    match origin {
        Some(value) if is_valid_chrome_extension_origin(value) => true,
        Some(value) if value.eq_ignore_ascii_case("null") => {
            fetch_site.is_some_and(|value| value.eq_ignore_ascii_case("none"))
        }
        None => fetch_site.is_some_and(|value| value.eq_ignore_ascii_case("none")),
        _ => false,
    }
}

fn is_valid_chrome_extension_origin(origin: &str) -> bool {
    let Some(extension_id) = origin.strip_prefix("chrome-extension://") else {
        return false;
    };

    extension_id.len() == 32
        && extension_id
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'p'))
}

#[cfg(test)]
mod tests {
    use actix_web::{http::header, test::TestRequest};

    use super::{RAIN_BROWSER_HEADER, is_rain_browser_extension_request};

    #[test]
    fn accepts_marked_chrome_extension_origin() {
        let request = TestRequest::post()
            .insert_header((
                header::ORIGIN,
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop",
            ))
            .insert_header((RAIN_BROWSER_HEADER, "1"))
            .to_srv_request();

        assert!(is_rain_browser_extension_request(&request));
    }

    #[test]
    fn accepts_marked_extension_fetch_with_null_origin_and_none_fetch_site() {
        let request = TestRequest::post()
            .insert_header((header::ORIGIN, "null"))
            .insert_header(("Sec-Fetch-Site", "none"))
            .insert_header((RAIN_BROWSER_HEADER, "1"))
            .to_srv_request();

        assert!(is_rain_browser_extension_request(&request));
    }

    #[test]
    fn accepts_marked_extension_fetch_without_origin_and_none_fetch_site() {
        let request = TestRequest::post()
            .insert_header(("Sec-Fetch-Site", "none"))
            .insert_header((RAIN_BROWSER_HEADER, "1"))
            .to_srv_request();

        assert!(is_rain_browser_extension_request(&request));
    }

    #[test]
    fn rejects_unmarked_extension_origin() {
        let request = TestRequest::post()
            .insert_header((
                header::ORIGIN,
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop",
            ))
            .to_srv_request();

        assert!(!is_rain_browser_extension_request(&request));
    }

    #[test]
    fn rejects_unmarked_none_fetch_site_request() {
        let request = TestRequest::post()
            .insert_header(("Sec-Fetch-Site", "none"))
            .to_srv_request();

        assert!(!is_rain_browser_extension_request(&request));
    }

    #[test]
    fn rejects_web_origin_even_with_extension_marker() {
        let request = TestRequest::post()
            .insert_header((header::ORIGIN, "https://example.com"))
            .insert_header(("Sec-Fetch-Site", "cross-site"))
            .insert_header((RAIN_BROWSER_HEADER, "1"))
            .to_srv_request();

        assert!(!is_rain_browser_extension_request(&request));
    }

    #[test]
    fn rejects_null_origin_when_fetch_site_is_cross_site() {
        let request = TestRequest::post()
            .insert_header((header::ORIGIN, "null"))
            .insert_header(("Sec-Fetch-Site", "cross-site"))
            .insert_header((RAIN_BROWSER_HEADER, "1"))
            .to_srv_request();

        assert!(!is_rain_browser_extension_request(&request));
    }
}
