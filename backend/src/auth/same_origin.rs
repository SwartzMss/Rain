use std::sync::OnceLock;

use actix_web::{
    Error, HttpResponse,
    body::{EitherBody, MessageBody},
    dev::{ServiceRequest, ServiceResponse},
    http::{Method, header},
    middleware::Next,
};

const RAIN_BROWSER_HEADER: &str = "x-rain-browser";
const RAIN_BROWSER_HEADER_VALUE: &str = "1";
const ALLOW_BROWSER_EXTENSION_REQUESTS_ENV: &str = "RAIN_ALLOW_BROWSER_EXTENSION_REQUESTS";
static ALLOW_BROWSER_EXTENSION_REQUESTS: OnceLock<bool> = OnceLock::new();

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
    is_rain_browser_extension_request_with_policy(request, browser_extension_requests_enabled())
}

fn is_rain_browser_extension_request_with_policy(
    request: &ServiceRequest,
    allow_browser_extension_requests: bool,
) -> bool {
    if !allow_browser_extension_requests {
        return false;
    }

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

fn browser_extension_requests_enabled() -> bool {
    *ALLOW_BROWSER_EXTENSION_REQUESTS.get_or_init(|| {
        std::env::var(ALLOW_BROWSER_EXTENSION_REQUESTS_ENV)
            .ok()
            .is_some_and(|value| parse_bool_flag(&value))
    })
}

fn parse_bool_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
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

    use super::{
        RAIN_BROWSER_HEADER, is_rain_browser_extension_request_with_policy, parse_bool_flag,
    };

    #[test]
    fn browser_extension_requests_are_disabled_by_default_policy() {
        let request = TestRequest::post()
            .insert_header((
                header::ORIGIN,
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop",
            ))
            .insert_header((RAIN_BROWSER_HEADER, "1"))
            .to_srv_request();

        assert!(!is_rain_browser_extension_request_with_policy(
            &request, false
        ));
    }

    #[test]
    fn accepts_marked_chrome_extension_origin_when_enabled() {
        let request = TestRequest::post()
            .insert_header((
                header::ORIGIN,
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop",
            ))
            .insert_header((RAIN_BROWSER_HEADER, "1"))
            .to_srv_request();

        assert!(is_rain_browser_extension_request_with_policy(
            &request, true
        ));
    }

    #[test]
    fn accepts_marked_extension_fetch_with_null_origin_and_none_fetch_site() {
        let request = TestRequest::post()
            .insert_header((header::ORIGIN, "null"))
            .insert_header(("Sec-Fetch-Site", "none"))
            .insert_header((RAIN_BROWSER_HEADER, "1"))
            .to_srv_request();

        assert!(is_rain_browser_extension_request_with_policy(
            &request, true
        ));
    }

    #[test]
    fn accepts_marked_extension_fetch_without_origin_and_none_fetch_site() {
        let request = TestRequest::post()
            .insert_header(("Sec-Fetch-Site", "none"))
            .insert_header((RAIN_BROWSER_HEADER, "1"))
            .to_srv_request();

        assert!(is_rain_browser_extension_request_with_policy(
            &request, true
        ));
    }

    #[test]
    fn rejects_unmarked_extension_request() {
        let request = TestRequest::post()
            .insert_header((
                header::ORIGIN,
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop",
            ))
            .to_srv_request();

        assert!(!is_rain_browser_extension_request_with_policy(
            &request, true
        ));
    }

    #[test]
    fn rejects_web_origin_even_with_extension_marker() {
        let request = TestRequest::post()
            .insert_header((header::ORIGIN, "https://example.com"))
            .insert_header(("Sec-Fetch-Site", "cross-site"))
            .insert_header((RAIN_BROWSER_HEADER, "1"))
            .to_srv_request();

        assert!(!is_rain_browser_extension_request_with_policy(
            &request, true
        ));
    }

    #[test]
    fn rejects_null_origin_when_fetch_site_is_cross_site() {
        let request = TestRequest::post()
            .insert_header((header::ORIGIN, "null"))
            .insert_header(("Sec-Fetch-Site", "cross-site"))
            .insert_header((RAIN_BROWSER_HEADER, "1"))
            .to_srv_request();

        assert!(!is_rain_browser_extension_request_with_policy(
            &request, true
        ));
    }

    #[test]
    fn parses_enabled_flag_values() {
        for value in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(parse_bool_flag(value), "{value}");
        }
        for value in ["", "0", "false", "off", "unexpected"] {
            assert!(!parse_bool_flag(value), "{value}");
        }
    }
}
