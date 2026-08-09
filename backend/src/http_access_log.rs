use std::time::Duration;

use actix_web::{
    Error, HttpMessage,
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    http::{Method, StatusCode},
    middleware::Next,
};
use uuid::Uuid;

use backend::RequestLogId;

const SLOW_REQUEST_THRESHOLD: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessLogLevel {
    Info,
    Warn,
    Error,
}

fn classify_access_log(
    method: &Method,
    status: StatusCode,
    elapsed: Duration,
) -> Option<AccessLogLevel> {
    if status.is_server_error() {
        Some(AccessLogLevel::Error)
    } else if status.is_client_error() || elapsed >= SLOW_REQUEST_THRESHOLD {
        Some(AccessLogLevel::Warn)
    } else if matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        Some(AccessLogLevel::Info)
    } else {
        None
    }
}

pub async fn log_useful_requests(
    request: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    let started = std::time::Instant::now();
    let request_id = Uuid::new_v4().simple().to_string();
    request
        .extensions_mut()
        .insert(RequestLogId(request_id.clone()));
    let method = request.method().clone();
    let path = request.path().to_string();
    let peer_ip = request
        .connection_info()
        .realip_remote_addr()
        .unwrap_or("unknown")
        .to_string();

    let response = match next.call(request).await {
        Ok(response) => response,
        Err(error) => {
            tracing::error!(
                target: "rain::http",
                %request_id,
                %method,
                %path,
                status = 500u16,
                elapsed_ms = started.elapsed().as_millis() as u64,
                %peer_ip,
                error = %error,
                "HTTP request failed"
            );
            return Err(error);
        }
    };

    let status = response.status();
    let elapsed = started.elapsed();
    match classify_access_log(&method, status, elapsed) {
        Some(AccessLogLevel::Error) => tracing::error!(
            target: "rain::http",
            %request_id,
            %method,
            %path,
            status = status.as_u16(),
            elapsed_ms = elapsed.as_millis() as u64,
            %peer_ip,
            "HTTP request completed"
        ),
        Some(AccessLogLevel::Warn) => tracing::warn!(
            target: "rain::http",
            %request_id,
            %method,
            %path,
            status = status.as_u16(),
            elapsed_ms = elapsed.as_millis() as u64,
            %peer_ip,
            "HTTP request completed"
        ),
        Some(AccessLogLevel::Info) => tracing::info!(
            target: "rain::http",
            %request_id,
            %method,
            %path,
            status = status.as_u16(),
            elapsed_ms = elapsed.as_millis() as u64,
            %peer_ip,
            "HTTP request completed"
        ),
        None => {}
    }

    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use actix_web::http::{Method, StatusCode};

    use super::{AccessLogLevel, classify_access_log};

    #[test]
    fn skips_fast_success_and_redirect_responses() {
        assert_eq!(
            classify_access_log(&Method::GET, StatusCode::OK, Duration::from_millis(999)),
            None
        );
        assert_eq!(
            classify_access_log(&Method::GET, StatusCode::FOUND, Duration::from_millis(50)),
            None
        );
    }

    #[test]
    fn warns_at_slow_request_threshold() {
        assert_eq!(
            classify_access_log(&Method::GET, StatusCode::OK, Duration::from_millis(1_000)),
            Some(AccessLogLevel::Warn)
        );
    }

    #[test]
    fn warns_for_client_errors() {
        assert_eq!(
            classify_access_log(
                &Method::GET,
                StatusCode::NOT_FOUND,
                Duration::from_millis(10)
            ),
            Some(AccessLogLevel::Warn)
        );
    }

    #[test]
    fn server_errors_take_precedence_over_latency() {
        assert_eq!(
            classify_access_log(
                &Method::GET,
                StatusCode::INTERNAL_SERVER_ERROR,
                Duration::from_secs(2)
            ),
            Some(AccessLogLevel::Error)
        );
    }

    #[test]
    fn logs_successful_mutations_at_info() {
        assert_eq!(
            classify_access_log(
                &Method::POST,
                StatusCode::ACCEPTED,
                Duration::from_millis(10)
            ),
            Some(AccessLogLevel::Info)
        );
        assert_eq!(
            classify_access_log(
                &Method::DELETE,
                StatusCode::NO_CONTENT,
                Duration::from_millis(10)
            ),
            Some(AccessLogLevel::Info)
        );
    }
}
