use std::time::Duration;

use super::client::ProviderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRequestStage {
    ModelRequest,
    FinalModelRequest,
    ResultRepair,
    ProviderTest,
    SkillReview,
    SkillReviewRepair,
}

impl ProviderRequestStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::ModelRequest => "model_request",
            Self::FinalModelRequest => "final_model_request",
            Self::ResultRepair => "result_repair",
            Self::ProviderTest => "provider_test",
            Self::SkillReview => "skill_review",
            Self::SkillReviewRepair => "skill_review_repair",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderRequestContext<'a> {
    pub stage: ProviderRequestStage,
    pub run_id: Option<&'a str>,
    pub iteration: Option<usize>,
    pub elapsed_ms: u64,
    pub tools_enabled: bool,
    pub tool_choice: Option<&'static str>,
    pub response_format: Option<&'static str>,
}

impl ProviderRequestContext<'static> {
    pub fn provider_test(elapsed_ms: u64) -> Self {
        Self {
            stage: ProviderRequestStage::ProviderTest,
            run_id: None,
            iteration: None,
            elapsed_ms,
            tools_enabled: false,
            tool_choice: None,
            response_format: None,
        }
    }

    pub fn skill_review(repair: bool, elapsed_ms: u64) -> Self {
        Self {
            stage: if repair {
                ProviderRequestStage::SkillReviewRepair
            } else {
                ProviderRequestStage::SkillReview
            },
            run_id: None,
            iteration: None,
            elapsed_ms,
            tools_enabled: false,
            tool_choice: None,
            response_format: Some("json_object"),
        }
    }
}

pub fn log_provider_failure(context: ProviderRequestContext<'_>, error: ProviderError) {
    log_provider_failure_attempt(context, error, 1, 1, false);
}

pub fn log_provider_failure_attempt(
    context: ProviderRequestContext<'_>,
    error: ProviderError,
    attempt: usize,
    max_attempts: usize,
    retry_exhausted: bool,
) {
    match error {
        ProviderError::HttpStatus { status, .. } => tracing::warn!(
            stage = %context.stage.as_str(),
            run_id = ?context.run_id,
            iteration = ?context.iteration,
            elapsed_ms = context.elapsed_ms,
            tools_enabled = context.tools_enabled,
            tool_choice = %context.tool_choice.unwrap_or("none"),
            response_format = %context.response_format.unwrap_or("none"),
            error_category = %error.category(),
            http_status = status,
            attempt,
            max_attempts,
            retry_exhausted,
            "AI provider request failed"
        ),
        ProviderError::Transport(reason) => tracing::warn!(
            stage = %context.stage.as_str(),
            run_id = ?context.run_id,
            iteration = ?context.iteration,
            elapsed_ms = context.elapsed_ms,
            tools_enabled = context.tools_enabled,
            tool_choice = %context.tool_choice.unwrap_or("none"),
            response_format = %context.response_format.unwrap_or("none"),
            error_category = %error.category(),
            reason = %reason.as_str(),
            attempt,
            max_attempts,
            retry_exhausted,
            "AI provider request failed"
        ),
        _ => tracing::warn!(
            stage = %context.stage.as_str(),
            run_id = ?context.run_id,
            iteration = ?context.iteration,
            elapsed_ms = context.elapsed_ms,
            tools_enabled = context.tools_enabled,
            tool_choice = %context.tool_choice.unwrap_or("none"),
            response_format = %context.response_format.unwrap_or("none"),
            error_category = %error.category(),
            attempt,
            max_attempts,
            retry_exhausted,
            "AI provider request failed"
        ),
    }
}

pub fn log_provider_retry(
    context: ProviderRequestContext<'_>,
    error: ProviderError,
    attempt: usize,
    max_attempts: usize,
    backoff: Duration,
) {
    let backoff_ms = backoff.as_millis() as u64;
    match error {
        ProviderError::HttpStatus { status, .. } => tracing::warn!(
            stage = %context.stage.as_str(),
            run_id = ?context.run_id,
            iteration = ?context.iteration,
            elapsed_ms = context.elapsed_ms,
            tools_enabled = context.tools_enabled,
            tool_choice = %context.tool_choice.unwrap_or("none"),
            response_format = %context.response_format.unwrap_or("none"),
            error_category = %error.category(),
            http_status = status,
            attempt,
            max_attempts,
            backoff_ms,
            "AI provider request will be retried"
        ),
        ProviderError::Transport(reason) => tracing::warn!(
            stage = %context.stage.as_str(),
            run_id = ?context.run_id,
            iteration = ?context.iteration,
            elapsed_ms = context.elapsed_ms,
            tools_enabled = context.tools_enabled,
            tool_choice = %context.tool_choice.unwrap_or("none"),
            response_format = %context.response_format.unwrap_or("none"),
            error_category = %error.category(),
            reason = %reason.as_str(),
            attempt,
            max_attempts,
            backoff_ms,
            "AI provider request will be retried"
        ),
        _ => tracing::warn!(
            stage = %context.stage.as_str(),
            run_id = ?context.run_id,
            iteration = ?context.iteration,
            elapsed_ms = context.elapsed_ms,
            tools_enabled = context.tools_enabled,
            tool_choice = %context.tool_choice.unwrap_or("none"),
            response_format = %context.response_format.unwrap_or("none"),
            error_category = %error.category(),
            attempt,
            max_attempts,
            backoff_ms,
            "AI provider request will be retried"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
    };

    use tracing_subscriber::fmt::MakeWriter;

    use super::{
        ProviderRequestContext, ProviderRequestStage, log_provider_failure,
        log_provider_failure_attempt, log_provider_retry,
    };
    use crate::ai_provider::client::{ProviderError, TransportReason};

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    struct SharedWriterGuard(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriterGuard {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> MakeWriter<'writer> for SharedWriter {
        type Writer = SharedWriterGuard;

        fn make_writer(&'writer self) -> Self::Writer {
            SharedWriterGuard(self.0.clone())
        }
    }

    fn capture_log(action: impl FnOnce()) -> String {
        let writer = SharedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, action);
        let bytes = writer.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn http_failure_log_preserves_safe_request_context_and_status() {
        let output = capture_log(|| {
            log_provider_failure(
                ProviderRequestContext {
                    stage: ProviderRequestStage::ModelRequest,
                    run_id: Some("run-1"),
                    iteration: Some(1),
                    elapsed_ms: 21_000,
                    tools_enabled: true,
                    tool_choice: Some("auto"),
                    response_format: None,
                },
                ProviderError::http(400),
            );
        });

        for expected in [
            "stage=model_request",
            "run_id=Some(\"run-1\")",
            "iteration=Some(1)",
            "elapsed_ms=21000",
            "tools_enabled=true",
            "tool_choice=auto",
            "error_category=http_status",
            "http_status=400",
        ] {
            assert!(output.contains(expected), "missing {expected} in {output}");
        }
    }

    #[test]
    fn transport_failure_log_contains_only_an_allow_listed_reason() {
        let error = ProviderError::Transport(TransportReason::ConnectionReset);
        let output = capture_log(|| {
            log_provider_failure(
                ProviderRequestContext {
                    stage: ProviderRequestStage::ResultRepair,
                    run_id: Some("run-2"),
                    iteration: None,
                    elapsed_ms: 12,
                    tools_enabled: false,
                    tool_choice: None,
                    response_format: Some("json_object"),
                },
                error,
            );
        });

        assert!(output.contains("stage=result_repair"));
        assert!(output.contains("error_category=transport"));
        assert!(output.contains("reason=connection_reset"));
        assert!(output.contains("response_format=json_object"));
        assert!(!output.contains("http_status"));
    }

    #[test]
    fn provider_failure_metadata_cannot_contain_sensitive_request_data() {
        let error = ProviderError::Transport(TransportReason::RequestFailed);
        let output = capture_log(|| {
            log_provider_failure(
                ProviderRequestContext {
                    stage: ProviderRequestStage::ProviderTest,
                    run_id: None,
                    iteration: None,
                    elapsed_ms: 8,
                    tools_enabled: false,
                    tool_choice: None,
                    response_format: None,
                },
                error,
            );
        });
        let rendered_error = error.to_string();

        for sensitive in [
            "sk-secret-value",
            "Authorization: Bearer secret",
            "https://user:password@provider.example/v1",
            "FULL PROMPT SENTINEL",
            "# SECRET SKILL MARKDOWN",
            "ISSUE LOG BODY SENTINEL",
            "UPSTREAM RESPONSE BODY SENTINEL",
        ] {
            assert!(!output.contains(sensitive));
            assert!(!rendered_error.contains(sensitive));
        }
        assert_eq!(error.category(), "transport");
        assert_eq!(error.http_status(), None);
        assert_eq!(error.transport_reason(), Some("request_failed"));
        assert_eq!(ProviderError::http(401).http_status(), Some(401));
    }

    #[test]
    fn provider_test_context_uses_a_safe_fixed_request_shape() {
        let output = capture_log(|| {
            log_provider_failure(
                ProviderRequestContext::provider_test(17),
                ProviderError::http(429),
            );
        });

        for expected in [
            "stage=provider_test",
            "run_id=None",
            "iteration=None",
            "elapsed_ms=17",
            "tools_enabled=false",
            "tool_choice=none",
            "response_format=none",
            "http_status=429",
        ] {
            assert!(output.contains(expected), "missing {expected} in {output}");
        }
    }

    #[test]
    fn skill_review_context_distinguishes_initial_and_repair_requests() {
        for (repair, expected_stage) in [
            (false, "stage=skill_review"),
            (true, "stage=skill_review_repair"),
        ] {
            let output = capture_log(|| {
                log_provider_failure(
                    ProviderRequestContext::skill_review(repair, 23),
                    ProviderError::InvalidResponse,
                );
            });
            for expected in [
                expected_stage,
                "elapsed_ms=23",
                "tools_enabled=false",
                "tool_choice=none",
                "response_format=json_object",
                "error_category=invalid_response",
            ] {
                assert!(output.contains(expected), "missing {expected} in {output}");
            }
        }
    }

    #[test]
    fn retry_log_contains_safe_attempt_and_backoff_fields() {
        let output = capture_log(|| {
            log_provider_retry(
                ProviderRequestContext {
                    stage: ProviderRequestStage::ResultRepair,
                    run_id: Some("run-2"),
                    iteration: None,
                    elapsed_ms: 12,
                    tools_enabled: false,
                    tool_choice: None,
                    response_format: Some("json_object"),
                },
                ProviderError::http(429),
                1,
                3,
                std::time::Duration::from_secs(1),
            );
        });

        for expected in [
            "stage=result_repair",
            "attempt=1",
            "max_attempts=3",
            "error_category=http_status",
            "http_status=429",
            "backoff_ms=1000",
        ] {
            assert!(output.contains(expected), "missing {expected} in {output}");
        }
    }

    #[test]
    fn exhausted_failure_log_is_explicit_and_safe() {
        let output = capture_log(|| {
            log_provider_failure_attempt(
                ProviderRequestContext::provider_test(17),
                ProviderError::Transport(TransportReason::ConnectionReset),
                3,
                3,
                true,
            );
        });

        for expected in [
            "attempt=3",
            "max_attempts=3",
            "retry_exhausted=true",
            "reason=connection_reset",
        ] {
            assert!(output.contains(expected), "missing {expected} in {output}");
        }
        for sensitive in ["Authorization", "FULL PROMPT", "UPSTREAM RESPONSE BODY"] {
            assert!(!output.contains(sensitive));
        }
    }
}
