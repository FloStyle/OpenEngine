//! Concrete `ModelAdapter` implementations, registered behind one config.
//!
//! Two OpenAI-compatible clients ship here:
//!
//! * [`deepseek::DeepSeekAdapter`] — a hosted `chat/completions` API keyed from
//!   an environment variable (`ProviderConfig::ApiKey`).
//! * [`llama_cpp::LlamaCppAdapter`] — a local OpenAI-compatible server
//!   (`ProviderConfig::Local`, e.g. `llama.cpp`/unsloth/vLLM), no auth.
//!
//! They share one request shape ([`openai`]) because both speak the same wire
//! protocol; the factory [`from_config`] maps the provider discriminant to the
//! right client so an assistant never branches on provider.

pub mod deepseek;
pub mod llama_cpp;
/// Model discovery + unsloth load/vision-check against a running server.
pub mod management;
pub(crate) mod openai;

use crate::{AdapterError, ModelAdapter, ModelConfig, ProviderConfig};

/// Build the client matching `config`.
///
/// `ApiKey` -> a remote [`deepseek::DeepSeekAdapter`]; `Local` -> a local
/// [`llama_cpp::LlamaCppAdapter`]. No network happens here — this only wires
/// the config to a concrete [`ModelAdapter`], so it is safe to call anywhere.
pub fn from_config(config: &ModelConfig) -> Result<Box<dyn ModelAdapter>, AdapterError> {
    match &config.provider {
        ProviderConfig::ApiKey { key_env, .. } => {
            // Fail fast on a missing key env var (a config error, not transport):
            // better to surface "set MY_KEY" than to send an unauthenticated
            // request that the provider rejects with a confusing 401.
            if std::env::var(key_env).is_err() {
                return Err(AdapterError::Config(format!(
                    "env var '{key_env}' (from provider config) is not set"
                )));
            }
            Ok(Box::new(deepseek::DeepSeekAdapter::new(config.clone())))
        }
        ProviderConfig::Local { .. } => {
            Ok(Box::new(llama_cpp::LlamaCppAdapter::new(config.clone())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_config() -> ModelConfig {
        ModelConfig {
            provider: ProviderConfig::ApiKey {
                endpoint: "https://api.deepseek.com/v1".into(),
                key_env: "OPENENGINE_TEST_KEY".into(),
            },
            model: "deepseek-chat".into(),
        }
    }

    fn local_config() -> ModelConfig {
        ModelConfig {
            provider: ProviderConfig::Local {
                endpoint: "http://127.0.0.1:8080/v1".into(),
                key_env: None,
            },
            model: "llama-3".into(),
        }
    }

    #[test]
    fn from_config_api_key_returns_deepseek_and_requires_the_key_env() {
        // Set the key env so the adapter's fail-fast check passes.
        std::env::set_var("OPENENGINE_TEST_KEY", "sk-test");
        let a = from_config(&api_config()).expect("deepseek adapter");
        // It reports the provider config we gave it (an ApiKey backend).
        assert!(matches!(a.config().provider, ProviderConfig::ApiKey { .. }));
        // It is a DeepSeek client (hosted API), not the local one.
        assert_eq!(a.name(), "deepseek");

        // Removing the key must now fail-fast with a config error. (Sequenced
        // here rather than in a parallel test: env vars are process-global.)
        std::env::remove_var("OPENENGINE_TEST_KEY");
        match from_config(&api_config()) {
            Err(AdapterError::Config(msg)) => assert!(msg.contains("OPENENGINE_TEST_KEY")),
            _ => panic!("expected a config error when the key env is unset"),
        }
    }

    #[test]
    fn from_config_local_returns_llama_cpp() {
        let a = from_config(&local_config()).expect("llama.cpp adapter");
        assert!(matches!(a.config().provider, ProviderConfig::Local { .. }));
        assert_eq!(a.name(), "llama.cpp");
    }
}
