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
        let endpoint = match &self.config.provider {
            crate::ProviderConfig::Local { endpoint } => endpoint,
            crate::ProviderConfig::ApiKey { .. } => {
                unreachable!("llama.cpp requires Local")
            }
        };
        // Local servers need no auth token.
        openai::chat(endpoint, None, &self.config.model, turns)
    }
}
