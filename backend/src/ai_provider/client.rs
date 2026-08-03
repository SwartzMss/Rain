use std::time::Duration;

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
    Transport,
    #[error("AI provider returned HTTP status {0}")]
    HttpStatus(u16),
    #[error("AI provider response exceeded the size limit")]
    ResponseTooLarge,
    #[error("AI provider returned an invalid response")]
    InvalidResponse,
}

impl ProviderError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Timeout => "AI_PROVIDER_TIMEOUT",
            Self::Transport => "AI_PROVIDER_UNAVAILABLE",
            Self::HttpStatus(_) => "AI_PROVIDER_HTTP_ERROR",
            Self::ResponseTooLarge => "AI_PROVIDER_RESPONSE_TOO_LARGE",
            Self::InvalidResponse => "AI_PROVIDER_INVALID_RESPONSE",
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
            .map_err(|_| ProviderError::Transport)?;
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
            .map_err(|error| {
                if error.is_timeout() {
                    ProviderError::Timeout
                } else {
                    ProviderError::Transport
                }
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(ProviderError::HttpStatus(status.as_u16()));
        }

        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ProviderError::Transport)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE_BYTES {
                return Err(ProviderError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        parse_chat_response(&bytes)
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
