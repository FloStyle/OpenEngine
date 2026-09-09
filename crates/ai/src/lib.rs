//! # Uniform model adapter (contract + concrete clients).
//!
//! Per `ADR-0002` the finished OpenEngine mounts an AI as a resident operator.
//! This crate fixes the *contract* an assistant talks through so that swapping
//! the model — a cloud API key or a local `llama.cpp`/unsloth endpoint — needs
//! **no assistant code change**.
//!
//! It ships the shared types + [`ModelAdapter`] trait, plus two concrete
//! OpenAI-compatible clients behind the [`providers`] factory
//! ([`providers::from_config`]): a DeepSeek API-key client and a local
//! llama.cpp client. Pick one with a [`ModelConfig`] and call [`ModelAdapter::complete`].

use serde::{Deserialize, Serialize};

/// Concrete model clients (`DeepSeek` API key + local `llama.cpp`), selected by
/// [`providers::from_config`]. Both speak OpenAI-compatible `chat/completions`.
pub mod providers;

/// Chat-message content models: plain text and multimodal parts.
pub mod content;

/// Resolved `ModelConfig` loading + catalog helpers.
pub mod config;

/// Minimal gitignored `.env` loader (no external dependency).
pub mod env;

pub use config::{resolve_config, ConfigSource};
pub use content::{ChatTurn, ContentPart, TextImageContent};
pub use env::load_dotenv;

/// Where + which model to talk to. `kind` discriminates a remote API key from a
/// local OpenAI-compatible endpoint (llama.cpp / unsloth / vLLM ...).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderConfig {
    /// A hosted chat-completions API authenticated with a key from the env.
    ApiKey {
        /// Base URL, e.g. `https://api.deepseek.com/v1`.
        endpoint: String,
        /// Name of the environment variable holding the key (never the key).
        key_env: String,
    },
    /// A local OpenAI-compatible server (llama.cpp / unsloth / vLLM).
    Local {
        /// e.g. `http://127.0.0.1:8080/v1`.
        endpoint: String,
        /// Optional env var holding a bearer key for a local server that
        /// requires auth (e.g. unsloth). `None` = no auth. `#[serde(default)]`
        /// keeps older configs (endpoint only) valid.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_env: Option<String>,
    },
}

/// Full model selection: provider + model name.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelConfig {
    pub provider: ProviderConfig,
    pub model: String,
}

impl ModelConfig {
    /// Parse a config from JSON (a small, explicit file — no fragile setup).
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// A human/agent-friendly one-line description (endpoint host + model).
    pub fn describe(&self) -> String {
        let host = match &self.provider {
            ProviderConfig::ApiKey { endpoint, .. } | ProviderConfig::Local { endpoint, .. } => {
                endpoint.clone()
            }
        };
        format!("{} @ {host}", self.model)
    }
}

/// Error surface an adapter reports (typed, so an AI gets structured feedback).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterError {
    Config(String),
    Transport(String),
    Status { code: u16, detail: String },
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdapterError::Config(m) => write!(f, "config: {m}"),
            AdapterError::Transport(m) => write!(f, "transport: {m}"),
            AdapterError::Status { code, detail } => write!(f, "http {code}: {detail}"),
        }
    }
}

impl std::error::Error for AdapterError {}

// `ChatTurn` + `ContentPart` live in `content.rs` (re-exported at the crate
// root via `pub use content::{...}`).

/// The uniform contract every provider (cloud key or local) satisfies.
///
/// Concrete clients live in [`providers`] and implement this trait; the
/// assistant code only ever sees the trait + a config, so switching providers
/// (DeepSeek API key vs local llama.cpp) needs no calling-code change.
pub trait ModelAdapter: Send + Sync {
    /// The resolved model + provider this adapter talks to.
    fn config(&self) -> &ModelConfig;

    /// Short provider label used to identify which concrete client an adapter
    /// is (`"deepseek"`, `"llama.cpp"`) without downcasting.
    fn name(&self) -> &str {
        "unknown"
    }

    /// Submit a conversation and return the assistant reply. Implemented by a
    /// concrete provider.
    fn complete(&self, turns: &[ChatTurn]) -> Result<String, AdapterError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_key_config() {
        let cfg = ModelConfig::from_json(
            r#"{"provider":{"kind":"api_key","endpoint":"https://api.deepseek.com/v1","key_env":"DSH_KEY"},"model":"deepseek-chat"}"#,
        )
        .unwrap();
        assert_eq!(
            cfg.provider,
            ProviderConfig::ApiKey {
                endpoint: "https://api.deepseek.com/v1".into(),
                key_env: "DSH_KEY".into()
            }
        );
        assert!(cfg.describe().contains("api.deepseek.com"));
    }

    #[test]
    fn parses_local_llama_config() {
        let cfg = ModelConfig::from_json(
            r#"{"provider":{"kind":"local","endpoint":"http://127.0.0.1:8080/v1"},"model":"llama-3"}"#,
        )
        .unwrap();
        assert_eq!(
            cfg.provider,
            ProviderConfig::Local {
                endpoint: "http://127.0.0.1:8080/v1".into(),
                key_env: None,
            }
        );
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert!(
            ModelConfig::from_json(r#"{"provider":{"kind":"space_magic"},"model":"x"}"#).is_err()
        );
    }
}
