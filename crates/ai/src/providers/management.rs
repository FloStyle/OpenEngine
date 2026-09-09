//! Model discovery + lifecycle against a running provider server.
//!
//! Two layers:
//! * OpenAI-standard discovery via `GET {endpoint}/v1/models` (works on any
//!   llama.cpp/unsloth/vLLM endpoint) — each row carries a `loaded` flag.
//! * Unsloth-Studio management (`/api/models/check-vision/{model}` and
//!   `/api/inference/load`) to *check* whether a model is a VLM and *load* it.
//!   Those endpoints are unsloth-specific; a non-unsloth server returns 404 and
//!   the helpers surface that as a typed error.

use crate::{AdapterError, ModelConfig, ProviderConfig};

/// A discovered model from `GET /v1/models`.
#[derive(Clone, Debug)]
pub struct DiscoveredModel {
    /// Full model id (repo/model).
    pub id: String,
    /// Whether it is currently loaded into the running server.
    pub loaded: bool,
}

fn base_of(cfg: &ModelConfig) -> Result<(&str, Option<&str>), AdapterError> {
    match &cfg.provider {
        ProviderConfig::ApiKey { endpoint, key_env } => Ok((endpoint, Some(key_env))),
        ProviderConfig::Local { endpoint, key_env } => Ok((endpoint, key_env.as_deref())),
    }
}

/// Resolve the bearer token from a key_env (None if none configured).
fn bearer(cfg: &ModelConfig, key_env: Option<&str>) -> Result<Option<String>, AdapterError> {
    let _ = cfg;
    match key_env {
        Some(env) => std::env::var(env)
            .map(Some)
            .map_err(|_| AdapterError::Config(format!("env '{env}' unset"))),
        None => Ok(None),
    }
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::new()
}

/// OpenAI-standard model discovery: `GET {endpoint}/v1/models`.
pub fn list_models(cfg: &ModelConfig) -> Result<Vec<DiscoveredModel>, AdapterError> {
    let (endpoint, ke) = base_of(cfg)?;
    let url = format!("{endpoint}/models");
    let mut rb = client().get(&url);
    if let Some(tok) = bearer(cfg, ke)? {
        rb = rb.bearer_auth(tok);
    }
    let resp = rb
        .send()
        .map_err(|e| AdapterError::Transport(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(AdapterError::Status {
            code: resp.status().as_u16(),
            detail: resp.text().unwrap_or_default(),
        });
    }
    let j: serde_json::Value = resp
        .json()
        .map_err(|e| AdapterError::Transport(format!("bad json: {e}")))?;
    let mut out = Vec::new();
    if let Some(arr) = j["data"].as_array() {
        for m in arr {
            let id = m["id"].as_str().unwrap_or("").to_string();
            let loaded = m["loaded"].as_bool().unwrap_or(false);
            if !id.is_empty() {
                out.push(DiscoveredModel { id, loaded });
            }
        }
    }
    Ok(out)
}

/// Unsloth-specific vision probe: `GET /api/models/check-vision/{model}`.
/// Non-unsloth servers 404 -> typed error so the caller can say "unsupported".
pub fn check_vision(cfg: &ModelConfig, model: &str) -> Result<bool, AdapterError> {
    let (endpoint, ke) = base_of(cfg)?;
    let host = endpoint.trim_end_matches('/');
    // Strip any trailing /v1 so we hit the studio root API.
    let root = host.strip_suffix("/v1").unwrap_or(host);
    let encoded = urlencode(model);
    let url = format!("{root}/api/models/check-vision/{encoded}");
    let mut rb = client().get(&url);
    if let Some(tok) = bearer(cfg, ke)? {
        rb = rb.bearer_auth(tok);
    }
    let resp = rb
        .send()
        .map_err(|e| AdapterError::Transport(e.to_string()))?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        return Err(AdapterError::Status {
            code,
            detail: format!("vision check unsupported here (http {code})"),
        });
    }
    let j: serde_json::Value = resp
        .json()
        .map_err(|e| AdapterError::Transport(format!("bad json: {e}")))?;
    Ok(j["is_vision"].as_bool().unwrap_or(false))
}

/// Unsloth-specific load: `POST /api/inference/load {model_path}`. Returns
/// whether the loaded model is a vision model. 404 on non-unsloth servers.
pub fn load_model(cfg: &ModelConfig, model: &str) -> Result<bool, AdapterError> {
    let (endpoint, ke) = base_of(cfg)?;
    let host = endpoint.trim_end_matches('/');
    let root = host.strip_suffix("/v1").unwrap_or(host);
    let url = format!("{root}/api/inference/load");
    let body = serde_json::json!({ "model_path": model });
    let mut rb = client().post(&url).json(&body);
    if let Some(tok) = bearer(cfg, ke)? {
        rb = rb.bearer_auth(tok);
    }
    let resp = rb
        .send()
        .map_err(|e| AdapterError::Transport(e.to_string()))?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let detail = resp.text().unwrap_or_default();
        return Err(AdapterError::Status {
            code,
            detail: detail.chars().take(200).collect(),
        });
    }
    let j: serde_json::Value = resp
        .json()
        .map_err(|e| AdapterError::Transport(format!("bad json: {e}")))?;
    Ok(j["is_vision"].as_bool().unwrap_or(false))
}

/// Percent-encode a model path for use in a URL path segment.
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_keeps_slashes_encoded() {
        // Slash in a model id must be percent-encoded in a path segment.
        assert_eq!(urlencode("a/b-c"), "a%2Fb-c");
        assert_eq!(urlencode("Qwen3.8-27B"), "Qwen3.8-27B");
    }

    #[test]
    fn root_strips_v1_for_management_api() {
        // check_vision builds host root by stripping trailing /v1.
        let host = "http://127.0.0.1:8889/v1";
        let root = host.strip_suffix("/v1").unwrap_or(host);
        assert_eq!(root, "http://127.0.0.1:8889");
    }
}
