use std::time::{Duration, Instant};

use super::{
    client::{ChatCompletionClient, ChatRequest, ChatResponse, ProviderError, TransportReason},
    observability::{ProviderRequestContext, log_provider_failure_attempt, log_provider_retry},
};

pub const MAX_PROVIDER_ATTEMPTS: usize = 3;
const DEFAULT_BACKOFFS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(2)];

fn is_retryable(error: ProviderError) -> bool {
    matches!(
        error,
        ProviderError::HttpStatus {
            status: 429 | 502 | 503 | 504,
            ..
        } | ProviderError::Transport(
            TransportReason::ConnectFailed | TransportReason::ConnectionReset
        )
    )
}

fn retry_delay(error: ProviderError, failed_attempt: usize) -> Duration {
    error
        .retry_after()
        .unwrap_or(DEFAULT_BACKOFFS[failed_attempt - 1])
}

pub async fn complete_with_retry(
    client: &dyn ChatCompletionClient,
    request: ChatRequest,
    mut context: ProviderRequestContext<'_>,
) -> Result<ChatResponse, ProviderError> {
    let started = Instant::now();
    for attempt in 1..=MAX_PROVIDER_ATTEMPTS {
        match client.complete(request.clone()).await {
            Ok(response) => return Ok(response),
            Err(error) => {
                context.elapsed_ms = started.elapsed().as_millis() as u64;
                let retryable = is_retryable(error);
                let exhausted = retryable && attempt == MAX_PROVIDER_ATTEMPTS;
                if retryable && !exhausted {
                    let backoff = retry_delay(error, attempt);
                    log_provider_retry(context, error, attempt, MAX_PROVIDER_ATTEMPTS, backoff);
                    tokio::time::sleep(backoff).await;
                    continue;
                }
                log_provider_failure_attempt(
                    context,
                    error,
                    attempt,
                    MAX_PROVIDER_ATTEMPTS,
                    exhausted,
                );
                return Err(error);
            }
        }
    }
    unreachable!("provider attempt loop always returns")
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use async_trait::async_trait;

    use super::{complete_with_retry, is_retryable, retry_delay};
    use crate::ai_provider::{
        client::{
            ChatCompletionClient, ChatMessage, ChatRequest, ChatResponse, ProviderError,
            TransportReason,
        },
        observability::{ProviderRequestContext, ProviderRequestStage},
    };

    struct ScriptedClient {
        responses: Mutex<VecDeque<Result<ChatResponse, ProviderError>>>,
        attempts: Arc<Mutex<usize>>,
    }

    impl ScriptedClient {
        fn new(responses: impl IntoIterator<Item = Result<ChatResponse, ProviderError>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                attempts: Arc::new(Mutex::new(0)),
            }
        }

        fn attempts(&self) -> usize {
            *self.attempts.lock().unwrap()
        }
    }

    #[async_trait]
    impl ChatCompletionClient for ScriptedClient {
        async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, ProviderError> {
            *self.attempts.lock().unwrap() += 1;
            self.responses.lock().unwrap().pop_front().unwrap()
        }
    }

    fn request() -> ChatRequest {
        ChatRequest {
            model: "test-model".into(),
            messages: Vec::new(),
            tools: Vec::new(),
            tool_choice: None,
            response_format: None,
        }
    }

    fn response() -> ChatResponse {
        ChatResponse {
            message: ChatMessage {
                role: "assistant".into(),
                content: Some("ok".into()),
                tool_calls: Vec::new(),
                tool_call_id: None,
                name: None,
            },
        }
    }

    fn context() -> ProviderRequestContext<'static> {
        ProviderRequestContext {
            stage: ProviderRequestStage::ResultRepair,
            run_id: Some("run-1"),
            iteration: None,
            elapsed_ms: 0,
            tools_enabled: false,
            tool_choice: None,
            response_format: Some("json_object"),
        }
    }

    #[test]
    fn retries_only_the_transient_allow_list() {
        for status in [429, 502, 503, 504] {
            assert!(is_retryable(ProviderError::http(status)));
        }
        for status in [400, 401, 403, 404, 500] {
            assert!(!is_retryable(ProviderError::http(status)));
        }
        for reason in [
            TransportReason::ConnectFailed,
            TransportReason::ConnectionReset,
        ] {
            assert!(is_retryable(ProviderError::Transport(reason)));
        }
        for error in [
            ProviderError::Timeout,
            ProviderError::Transport(TransportReason::DnsFailed),
            ProviderError::Transport(TransportReason::TlsFailed),
            ProviderError::Transport(TransportReason::RequestFailed),
            ProviderError::InvalidResponse,
            ProviderError::ResponseTooLarge,
        ] {
            assert!(!is_retryable(error));
        }
    }

    #[test]
    fn uses_retry_after_before_default_exponential_backoff() {
        assert_eq!(
            retry_delay(ProviderError::http(429), 1),
            Duration::from_secs(1)
        );
        assert_eq!(
            retry_delay(ProviderError::http(503), 2),
            Duration::from_secs(2)
        );
        assert_eq!(
            retry_delay(
                ProviderError::http_with_retry_after(429, Duration::from_millis(250)),
                1,
            ),
            Duration::from_millis(250)
        );
    }

    #[tokio::test]
    async fn retries_once_then_returns_a_successful_response() {
        let client = ScriptedClient::new([
            Err(ProviderError::http_with_retry_after(429, Duration::ZERO)),
            Ok(response()),
        ]);

        let result = complete_with_retry(&client, request(), context()).await;

        assert_eq!(result.unwrap(), response());
        assert_eq!(client.attempts(), 2);
    }

    #[tokio::test]
    async fn does_not_retry_a_non_retryable_failure() {
        let client = ScriptedClient::new([Err(ProviderError::http(400))]);

        let result = complete_with_retry(&client, request(), context()).await;

        assert_eq!(result.unwrap_err(), ProviderError::http(400));
        assert_eq!(client.attempts(), 1);
    }

    #[tokio::test]
    async fn stops_after_three_retryable_failures() {
        let failure = ProviderError::http_with_retry_after(503, Duration::ZERO);
        let client = ScriptedClient::new([Err(failure), Err(failure), Err(failure)]);

        let result = complete_with_retry(&client, request(), context()).await;

        assert_eq!(result.unwrap_err(), failure);
        assert_eq!(client.attempts(), 3);
    }
}
