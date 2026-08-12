use std::{
    error::Error as _,
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{HeaderValue, RETRY_AFTER};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::config::ResolvedAiProvider;

const MAX_PROVIDER_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatFunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ChatFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ChatToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    pub message: ChatMessage,
}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("AI provider request timed out")]
    Timeout,
    #[error("AI provider request failed")]
    Transport(TransportReason),
    #[error("AI provider returned HTTP status {status}")]
    HttpStatus {
        status: u16,
        retry_after: Option<Duration>,
    },
    #[error("AI provider response exceeded the size limit")]
    ResponseTooLarge,
    #[error("AI provider returned an invalid response")]
    InvalidResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportReason {
    ConnectFailed,
    DnsFailed,
    TlsFailed,
    ConnectionReset,
    RequestFailed,
}

impl TransportReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConnectFailed => "connect_failed",
            Self::DnsFailed => "dns_failed",
            Self::TlsFailed => "tls_failed",
            Self::ConnectionReset => "connection_reset",
            Self::RequestFailed => "request_failed",
        }
    }
}

impl ProviderError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Timeout => "AI_PROVIDER_TIMEOUT",
            Self::Transport(_) => "AI_PROVIDER_UNAVAILABLE",
            Self::HttpStatus { .. } => "AI_PROVIDER_HTTP_ERROR",
            Self::ResponseTooLarge => "AI_PROVIDER_RESPONSE_TOO_LARGE",
            Self::InvalidResponse => "AI_PROVIDER_INVALID_RESPONSE",
        }
    }

    pub fn category(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Transport(_) => "transport",
            Self::HttpStatus { .. } => "http_status",
            Self::ResponseTooLarge => "response_too_large",
            Self::InvalidResponse => "invalid_response",
        }
    }

    pub fn http_status(self) -> Option<u16> {
        match self {
            Self::HttpStatus { status, .. } => Some(status),
            _ => None,
        }
    }

    pub fn transport_reason(self) -> Option<&'static str> {
        match self {
            Self::Transport(reason) => Some(reason.as_str()),
            _ => None,
        }
    }

    pub fn http(status: u16) -> Self {
        Self::HttpStatus {
            status,
            retry_after: None,
        }
    }

    pub fn http_with_retry_after(status: u16, retry_after: Duration) -> Self {
        Self::HttpStatus {
            status,
            retry_after: Some(retry_after),
        }
    }

    pub fn retry_after(self) -> Option<Duration> {
        match self {
            Self::HttpStatus { retry_after, .. } => retry_after,
            _ => None,
        }
    }
}

#[async_trait]
pub trait ChatCompletionClient: Send + Sync {
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError>;
}

pub struct OpenAiChatClient {
    http: reqwest::Client,
    endpoint: String,
    api_key: String,
    timeout: Duration,
    model: String,
}

impl OpenAiChatClient {
    pub fn new(config: &ResolvedAiProvider) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|_| ProviderError::Transport(TransportReason::RequestFailed))?;
        Ok(Self {
            http,
            endpoint: format!("{}/chat/completions", config.base_url.trim_end_matches('/')),
            api_key: config.api_key().to_owned(),
            timeout: Duration::from_secs(config.timeout_seconds),
            model: config.model.clone(),
        })
    }
}

#[async_trait]
impl ChatCompletionClient for OpenAiChatClient {
    async fn complete(&self, mut request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        if request.model.is_empty() {
            request.model.clone_from(&self.model);
        }
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .timeout(self.timeout)
            .json(&request)
            .send()
            .await
            .map_err(provider_request_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(ProviderError::HttpStatus {
                status: status.as_u16(),
                retry_after: parse_retry_after(
                    response.headers().get(RETRY_AFTER),
                    SystemTime::now(),
                ),
            });
        }

        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(provider_request_error)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE_BYTES {
                return Err(ProviderError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        parse_chat_response(&bytes)
    }
}

fn parse_retry_after(value: Option<&HeaderValue>, now: SystemTime) -> Option<Duration> {
    let value = value?.to_str().ok()?;
    let duration = if let Ok(seconds) = value.parse::<u64>() {
        Duration::from_secs(seconds)
    } else {
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(now)
            .ok()?
    };
    Some(duration.min(MAX_RETRY_AFTER))
}

fn provider_request_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::Transport(classify_transport_reason(&error))
    }
}

fn classify_transport_reason(error: &reqwest::Error) -> TransportReason {
    let mut source = error.source();
    let mut dns_failed = false;
    let mut tls_failed = false;
    let mut connection_reset = false;
    while let Some(cause) = source {
        let message = cause.to_string().to_ascii_lowercase();
        tls_failed |= contains_rustls_error(cause);
        dns_failed |= message.contains("dns")
            || message.contains("name resolution")
            || message.contains("failed to lookup address");
        tls_failed |= message.contains("tls")
            || message.contains("rustls")
            || message.contains("certificate");
        connection_reset |= message.contains("connection reset");
        source = cause.source();
    }
    if dns_failed {
        TransportReason::DnsFailed
    } else if tls_failed {
        TransportReason::TlsFailed
    } else if connection_reset {
        TransportReason::ConnectionReset
    } else if error.is_connect() {
        TransportReason::ConnectFailed
    } else {
        TransportReason::RequestFailed
    }
}

fn contains_rustls_error(error: &(dyn std::error::Error + 'static)) -> bool {
    if error.is::<rustls::Error>() {
        return true;
    }
    error
        .downcast_ref::<std::io::Error>()
        .and_then(std::io::Error::get_ref)
        .is_some_and(|inner| contains_rustls_error(inner))
}

#[derive(Deserialize)]
struct CompletionEnvelope {
    choices: Vec<CompletionChoice>,
}

#[derive(Deserialize)]
struct CompletionChoice {
    message: ChatMessage,
}

pub fn parse_chat_response(bytes: &[u8]) -> Result<ChatResponse, ProviderError> {
    let envelope: CompletionEnvelope =
        serde_json::from_slice(bytes).map_err(|_| ProviderError::InvalidResponse)?;
    let message = envelope
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message)
        .ok_or(ProviderError::InvalidResponse)?;
    if message.role != "assistant" {
        return Err(ProviderError::InvalidResponse);
    }
    Ok(ChatResponse { message })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use reqwest::header::HeaderValue;

    use super::parse_retry_after;

    #[test]
    fn retry_after_parses_seconds_and_caps_large_values() {
        let three = HeaderValue::from_static("3");
        let thirty = HeaderValue::from_static("30");

        assert_eq!(
            parse_retry_after(Some(&three), SystemTime::UNIX_EPOCH),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            parse_retry_after(Some(&thirty), SystemTime::UNIX_EPOCH),
            Some(Duration::from_secs(10))
        );
    }

    #[test]
    fn retry_after_parses_future_http_date() {
        let value = HeaderValue::from_static("Thu, 01 Jan 1970 00:00:04 GMT");

        assert_eq!(
            parse_retry_after(Some(&value), SystemTime::UNIX_EPOCH),
            Some(Duration::from_secs(4))
        );
    }

    #[test]
    fn retry_after_rejects_invalid_and_expired_values() {
        let invalid = HeaderValue::from_static("invalid");
        let expired = HeaderValue::from_static("Thu, 01 Jan 1970 00:00:04 GMT");

        assert_eq!(
            parse_retry_after(Some(&invalid), SystemTime::UNIX_EPOCH),
            None
        );
        assert_eq!(
            parse_retry_after(
                Some(&expired),
                SystemTime::UNIX_EPOCH + Duration::from_secs(5)
            ),
            None
        );
        assert_eq!(parse_retry_after(None, SystemTime::UNIX_EPOCH), None);
    }
}
