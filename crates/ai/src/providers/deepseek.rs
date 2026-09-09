//! DeepSeek client — a hosted `chat/completions` API keyed from an env var.
//!
//! Selected by [`super::from_config`] for a [`ProviderConfig::ApiKey`]. The key
//! is read from the environment (`key_env`) at construction time by the
//! factory's fail-fast check and resolved again on each call, so a rotated key
//! is picked up without re-wiring the adapter.

use crate::{AdapterError, ChatTurn, ModelAdapter, ModelConfig, ProviderConfig};

use super::openai;

/// A hosted DeepSeek (OpenAI-compatible) client.
pub struct DeepSeekAdapter {
    config: ModelConfig,
}

impl DeepSeekAdapter {
    /// Wrap an `ApiKey` config into a DeepSeek client.
    pub fn new(config: ModelConfig) -> Self {
        DeepSeekAdapter { config }
    }

    /// The environment variable holding this provider's API key.
    fn key_env(&self) -> &str {
        match &self.config.provider {
            ProviderConfig::ApiKey { key_env, .. } => key_env,
            ProviderConfig::Local { .. } => unreachable!("DeepSeek requires ApiKey"),
        }
    }
}

impl ModelAdapter for DeepSeekAdapter {
    fn config(&self) -> &ModelConfig {
        &self.config
    }

    fn name(&self) -> &str {
        "deepseek"
    }

    fn complete(&self, turns: &[ChatTurn]) -> Result<String, AdapterError> {
        let key = std::env::var(self.key_env())
            .map_err(|_| AdapterError::Config(format!("env '{}' unset", self.key_env())))?;
        let endpoint = match &self.config.provider {
            ProviderConfig::ApiKey { endpoint, .. } => endpoint,
            ProviderConfig::Local { .. } => unreachable!("DeepSeek requires ApiKey"),
        };
        openai::chat(endpoint, Some(&key), &self.config.model, turns)
    }
}
