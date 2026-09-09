//! Integration: a key in a gitignored `.env` makes `from_config` succeed; a
//! missing key yields the typed Config error. Uses a unique env var name so it
//! never collides with real keys in parallel CI.

use openengine_ai::providers::from_config;
use openengine_ai::{AdapterError, ModelConfig, ProviderConfig};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_var() -> String {
    format!(
        "OE_DOTENV_TEST_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

fn config_for(var: &str) -> ModelConfig {
    ModelConfig {
        provider: ProviderConfig::ApiKey {
            endpoint: "https://api.deepseek.com/v1".into(),
            key_env: var.to_string(),
        },
        model: "deepseek-chat".into(),
    }
}

#[test]
fn from_config_uses_env_set_from_env_file() {
    // Simulate the .env loader having populated the process env: a real env var
    // named by key_env must make from_config succeed (it reads key_env).
    let var = unique_var();
    std::env::set_var(&var, "sk-dummy");
    let adapter = from_config(&config_for(&var)).expect("adapter builds when key env is set");
    assert_eq!(adapter.name(), "deepseek");
    std::env::remove_var(&var);
}

#[test]
fn from_config_missing_key_is_typed_config_error() {
    let var = unique_var();
    std::env::remove_var(&var);
    match from_config(&config_for(&var)) {
        Err(AdapterError::Config(msg)) => assert!(msg.contains(&var)),
        _ => panic!("expected Config error for missing key env"),
    }
}
