use std::{error::Error as _, time::Duration};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::config::ResolvedAiProvider;

const MAX_PROVIDER_RESPONSE_BYTES: usize = 1024 * 1024;

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
    #[error("AI provider returned HTTP status {0}")]
    HttpStatus(u16),
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
            Self::HttpStatus(_) => "AI_PROVIDER_HTTP_ERROR",
            Self::ResponseTooLarge => "AI_PROVIDER_RESPONSE_TOO_LARGE",
            Self::InvalidResponse => "AI_PROVIDER_INVALID_RESPONSE",
        }
    }

    pub fn category(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Transport(_) => "transport",
            Self::HttpStatus(_) => "http_status",
            Self::ResponseTooLarge => "response_too_large",
            Self::InvalidResponse => "invalid_response",
        }
    }

    pub fn http_status(self) -> Option<u16> {
        match self {
            Self::HttpStatus(status) => Some(status),
            _ => None,
        }
    }

    pub fn transport_reason(self) -> Option<&'static str> {
        match self {
            Self::Transport(reason) => Some(reason.as_str()),
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
            return Err(ProviderError::HttpStatus(status.as_u16()));
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
