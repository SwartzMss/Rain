use std::time::Duration;

use actix_web::{
    Error,
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    http::StatusCode,
    middleware::Next,
};

const SLOW_REQUEST_THRESHOLD: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessLogLevel {
    Warn,
    Error,
}

fn classify_access_log(status: StatusCode, elapsed: Duration) -> Option<AccessLogLevel> {
    if status.is_server_error() {
        Some(AccessLogLevel::Error)
    } else if status.is_client_error() || elapsed >= SLOW_REQUEST_THRESHOLD {
        Some(AccessLogLevel::Warn)
    } else {
        None
    }
}

pub async fn log_useful_requests(
    request: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    let started = std::time::Instant::now();
    let method = request.method().clone();
    let path = request.uri().to_string();
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
    match classify_access_log(status, elapsed) {
        Some(AccessLogLevel::Error) => tracing::error!(
            target: "rain::http",
            %method,
            %path,
            status = status.as_u16(),
            elapsed_ms = elapsed.as_millis() as u64,
            %peer_ip,
            "HTTP request completed"
        ),
        Some(AccessLogLevel::Warn) => tracing::warn!(
            target: "rain::http",
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

    use actix_web::http::StatusCode;

    use super::{AccessLogLevel, classify_access_log};

    #[test]
    fn skips_fast_success_and_redirect_responses() {
        assert_eq!(
            classify_access_log(StatusCode::OK, Duration::from_millis(999)),
            None
        );
        assert_eq!(
            classify_access_log(StatusCode::FOUND, Duration::from_millis(50)),
            None
        );
    }

    #[test]
    fn warns_at_slow_request_threshold() {
        assert_eq!(
            classify_access_log(StatusCode::OK, Duration::from_millis(1_000)),
            Some(AccessLogLevel::Warn)
        );
    }

    #[test]
    fn warns_for_client_errors() {
        assert_eq!(
            classify_access_log(StatusCode::NOT_FOUND, Duration::from_millis(10)),
            Some(AccessLogLevel::Warn)
        );
    }

    #[test]
    fn server_errors_take_precedence_over_latency() {
        assert_eq!(
            classify_access_log(StatusCode::INTERNAL_SERVER_ERROR, Duration::from_secs(2)),
            Some(AccessLogLevel::Error)
        );
    }
}
