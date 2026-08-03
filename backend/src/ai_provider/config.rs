use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

use crate::{config::AiProviderEnv, error::AppError};

use super::crypto::SecretCipher;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProviderSource {
    Database,
    Environment,
}

#[derive(Clone)]
pub struct ResolvedAiProvider {
    pub source: ProviderSource,
    pub base_url: String,
    api_key: String,
    pub model: String,
    pub timeout_seconds: u64,
}

impl std::fmt::Debug for ResolvedAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedAiProvider")
            .field("source", &self.source)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("timeout_seconds", &self.timeout_seconds)
            .finish()
    }
}

impl ResolvedAiProvider {
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub(crate) fn candidate(
        source: ProviderSource,
        base_url: String,
        api_key: String,
        model: String,
        timeout_seconds: u64,
    ) -> Self {
        Self {
            source,
            base_url,
            api_key,
            model,
            timeout_seconds,
        }
    }
}

#[derive(FromRow)]
struct StoredProvider {
    base_url: String,
    encrypted_api_key: String,
    model: String,
    request_timeout_seconds: i64,
}

pub async fn resolve_effective_config(
    pool: &SqlitePool,
    env: &AiProviderEnv,
) -> Result<Option<ResolvedAiProvider>, AppError> {
    let stored = sqlx::query_as::<_, StoredProvider>(
        "SELECT base_url,encrypted_api_key,model,request_timeout_seconds FROM ai_provider_settings WHERE id=1",
    )
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?;

    if let (Some(stored), Some(master_key)) = (stored, env.master_key)
        && let Ok(api_key) = SecretCipher::new(master_key).decrypt(&stored.encrypted_api_key)
        && !stored.base_url.trim().is_empty()
        && !stored.model.trim().is_empty()
        && !api_key.trim().is_empty()
        && (1..=300).contains(&stored.request_timeout_seconds)
    {
        return Ok(Some(ResolvedAiProvider {
            source: ProviderSource::Database,
            base_url: stored.base_url,
            api_key,
            model: stored.model,
            timeout_seconds: stored.request_timeout_seconds as u64,
        }));
    }

    if env.environment_provider_is_complete() {
        return Ok(Some(ResolvedAiProvider {
            source: ProviderSource::Environment,
            base_url: env.base_url.clone().expect("complete provider base URL"),
            api_key: env.api_key().expect("complete provider API key").to_owned(),
            model: env.model.clone().expect("complete provider model"),
            timeout_seconds: env.timeout_seconds,
        }));
    }

    Ok(None)
}
