//! Resolving a [`ModelConfig`] from files/env, plus the model catalog.

use std::path::Path;

use crate::{AdapterError, ModelConfig};

/// Where a config was resolved from (used for error messages / describe output).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    /// `--config <path>` flag (highest priority).
    Flag(String),
    /// `OPENENGINE_AI_CONFIG` env var.
    Env,
    /// `./config/ai.json` fallback.
    Default,
}

impl std::fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigSource::Flag(p) => write!(f, "--config {p}"),
            ConfigSource::Env => write!(f, "$OPENENGINE_AI_CONFIG"),
            ConfigSource::Default => write!(f, "./config/ai.json"),
        }
    }
}

/// Resolve a config using the documented order:
/// `--config <path>` > `$OPENENGINE_AI_CONFIG` > `./config/ai.json`.
///
/// Returns a typed [`AdapterError::Config`] when nothing resolves, with help
/// text naming all three sources.
pub fn resolve_config(
    flag_path: Option<&str>,
) -> Result<(ModelConfig, ConfigSource), AdapterError> {
    if let Some(p) = flag_path {
        let cfg = load_file(p)?;
        return Ok((cfg, ConfigSource::Flag(p.to_string())));
    }
    if let Ok(p) = std::env::var("OPENENGINE_AI_CONFIG") {
        if !p.is_empty() {
            let cfg = load_file(&p)?;
            return Ok((cfg, ConfigSource::Env));
        }
    }
    const DEFAULT: &str = "./config/ai.json";
    if Path::new(DEFAULT).exists() {
        let cfg = load_file(DEFAULT)?;
        return Ok((cfg, ConfigSource::Default));
    }
    Err(AdapterError::Config(
        "no AI config found. Provide --config <path>, set OPENENGINE_AI_CONFIG, \
         or create ./config/ai.json (see config/ai.example.json)."
            .to_string(),
    ))
}

fn load_file(path: &str) -> Result<ModelConfig, AdapterError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| AdapterError::Config(format!("cannot read {path}: {e}")))?;
    ModelConfig::from_json(&text)
        .map_err(|e| AdapterError::Config(format!("bad config {path}: {e}")))
}

/// A catalog row describing a known model / provider capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    /// Provider kind label: `"deepseek"` or `"llama.cpp"`.
    pub provider_kind: &'static str,
    /// Model id, e.g. `"deepseek-chat"`.
    pub id: &'static str,
    /// Human display name.
    pub display_name: &'static str,
    /// Whether the model accepts image inputs.
    pub vision: bool,
    /// Approx. context window in tokens.
    pub context_window: u32,
}

/// Built-in catalog. Local llama.cpp/unsloth entries are advisory (the actual
/// served model list comes from the running server); the `vision` flag for a
/// local endpoint is set by config (`ProviderConfig::Local` has no vision field
/// today, so local entries are listed non-vision unless the caller overrides).
pub const CATALOG: &[ModelInfo] = &[
    ModelInfo {
        provider_kind: "deepseek",
        id: "deepseek-chat",
        display_name: "DeepSeek Chat",
        vision: false,
        context_window: 64_000,
    },
    ModelInfo {
        provider_kind: "deepseek",
        id: "deepseek-reasoner",
        display_name: "DeepSeek Reasoner",
        vision: false,
        context_window: 64_000,
    },
    ModelInfo {
        provider_kind: "openai-compat",
        id: "gpt-4o-vision",
        display_name: "OpenAI-compatible vision (generic)",
        vision: true,
        context_window: 128_000,
    },
    ModelInfo {
        provider_kind: "llama.cpp",
        id: "local-llama",
        display_name: "Local llama.cpp/unsloth endpoint",
        vision: false,
        context_window: 8_192,
    },
];

/// Look up a catalog entry by provider kind + model id (best effort).
pub fn find_catalog(provider_kind: &str, id: &str) -> Option<&'static ModelInfo> {
    CATALOG
        .iter()
        .find(|m| m.provider_kind == provider_kind && m.id == id)
}

/// Whether a config points at a vision-capable model.
///
/// A remote DeepSeek model is never vision; an OpenAI-compatible vision model
/// (or a local server you configured as vision) may be. Since local endpoints
/// can carry a vision model loaded from `mmproj`, callers may pass an explicit
/// override for local configs; the base heuristic is catalog-driven.
pub fn vision_supported(cfg: &ModelConfig) -> bool {
    match &cfg.provider {
        crate::ProviderConfig::ApiKey { .. } => {
            // DeepSeek has no vision today; treat as non-vision unless a
            // catalog vision id is used on a generic key endpoint.
            find_catalog("openai-compat", &cfg.model)
                .map(|m| m.vision)
                .unwrap_or(false)
        }
        crate::ProviderConfig::Local { .. } => {
            // Local: assume non-vision (must be a VLM with mmproj to see).
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderConfig;

    fn local_cfg(model: &str) -> ModelConfig {
        ModelConfig {
            provider: ProviderConfig::Local {
                endpoint: "http://127.0.0.1:8889/v1".into(),
                key_env: None,
            },
            model: model.into(),
        }
    }

    #[test]
    fn resolve_uses_flag_then_env_then_default() {
        // No flag/env/default -> typed Config error with help.
        std::env::remove_var("OPENENGINE_AI_CONFIG");
        let err = resolve_config(None).unwrap_err();
        match err {
            AdapterError::Config(msg) => {
                assert!(msg.contains("--config") && msg.contains("OPENENGINE_AI_CONFIG"))
            }
            other => panic!("expected Config error, got {other:?}"),
        }
        // With a flag pointing at a temp file -> resolves to Local.
        let tmp = std::env::temp_dir().join("ai_cfg_test.json");
        std::fs::write(
            &tmp,
            r#"{"provider":{"kind":"local","endpoint":"http://127.0.0.1:8889/v1"},"model":"m"}"#,
        )
        .unwrap();
        let (cfg, src) = resolve_config(Some(tmp.to_str().unwrap())).unwrap();
        assert_eq!(src, ConfigSource::Flag(tmp.to_str().unwrap().to_string()));
        assert!(matches!(cfg.provider, ProviderConfig::Local { .. }));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn catalog_and_vision_heuristic() {
        // DeepSeek chat: not vision.
        let ds = ModelConfig {
            provider: ProviderConfig::ApiKey {
                endpoint: "https://api.deepseek.com/v1".into(),
                key_env: "DEEPSEEK_API_KEY".into(),
            },
            model: "deepseek-chat".into(),
        };
        assert!(!vision_supported(&ds));
        // Local llama: heuristic non-vision (VLM would need mmproj/config flag).
        assert!(!vision_supported(&local_cfg("any")));
        // Catalog lists a vision-capable openai-compat model.
        assert!(CATALOG.iter().any(|m| m.id == "gpt-4o-vision" && m.vision));
        assert!(find_catalog("deepseek", "deepseek-chat").is_some());
    }
}
