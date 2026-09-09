//! Local llama.cpp client — an OpenAI-compatible endpoint with no auth.
//!
//! Selected by [`super::from_config`] for a [`ProviderConfig::Local`]. Works
//! with any OpenAI-compatible local server (`llama.cpp` `llama-server`,
//! unsloth, vLLM ...) that exposes `{endpoint}/chat/completions`.

use crate::{AdapterError, ChatTurn, ModelAdapter, ModelConfig};

use super::openai;

/// A local llama.cpp / OpenAI-compatible client.
pub struct LlamaCppAdapter {
    config: ModelConfig,
}

impl LlamaCppAdapter {
    /// Wrap a `Local` config into a llama.cpp client.
    pub fn new(config: ModelConfig) -> Self {
        LlamaCppAdapter { config }
    }
}

impl ModelAdapter for LlamaCppAdapter {
    fn config(&self) -> &ModelConfig {
        &self.config
    }

    fn name(&self) -> &str {
        "llama.cpp"
    }

    fn complete(&self, turns: &[ChatTurn]) -> Result<String, AdapterError> {
        let (endpoint, key_env) = match &self.config.provider {
            crate::ProviderConfig::Local { endpoint, key_env } => (endpoint, key_env.as_deref()),
            crate::ProviderConfig::ApiKey { .. } => {
                unreachable!("llama.cpp requires Local")
            }
        };
        // Some local servers (e.g. unsloth) still require a bearer key.
        let bearer = match key_env {
            Some(env) => Some(
                std::env::var(env)
                    .map_err(|_| AdapterError::Config(format!("env '{env}' unset")))?,
            ),
            None => None,
        };
        openai::chat(endpoint, bearer.as_deref(), &self.config.model, turns)
    }
}
