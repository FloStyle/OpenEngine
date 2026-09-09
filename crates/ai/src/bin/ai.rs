//! `openengine-ai` CLI — pick a model (config), ping it, chat, and (multimodal)
//! describe a live harness screenshot. The "pick a model and test" door.
//!
//! Subcommands:
//!   describe --config <p>        show resolved provider/model/endpoint
//!   providers                    list built-in provider kinds
//!   models                       catalog table (id, vision, context)
//!   test    --config <p>         1-turn ping; print provider/model/ms/reply
//!   chat    --config <p> "msg" [--system "s"] [--image file.png]
//!   see     --config <p> [--harness http://127.0.0.1:8080] ["prompt"]
//!
//! Config resolution: --config > $OPENENGINE_AI_CONFIG > ./config/ai.json.
//! Keys are always env-derived (`key_env`) — never in the file.

use std::process::ExitCode;

use base64::Engine;
use openengine_ai::config::{resolve_config, ConfigSource, CATALOG};
use openengine_ai::content::{ChatTurn, TextImageContent};
use openengine_ai::providers::from_config;
use openengine_ai::{AdapterError, ModelConfig};

const USAGE: &str = "\
openengine-ai <cmd> [--config <path>] [--model <id>] [args]

commands:
  describe                print resolved provider/model/endpoint
  providers               list built-in provider kinds
  models                  model catalog table (id, vision, context)
  list                    DISCOVER models on the running endpoint (/v1/models)
  check <model>           ask the endpoint whether <model> is vision-capable
  load <model>            load <model> on the endpoint (unsloth inference API)
  test                    one-turn ping (key + model test)
  chat <msg> [--system <s>] [--image <file>]
  see [--harness <url>] [--editor] [<prompt>]   describe a live /frame or the
                                               real editor window (--editor)
config: --config <path> | $OPENENGINE_AI_CONFIG | ./config/ai.json";

struct Args {
    cmd: String,
    config: Option<String>,
    model: Option<String>,
    positionals: Vec<String>,
    system: Option<String>,
    image: Option<String>,
    harness: String,
    editor: bool,
}

fn parse() -> Result<Args, String> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut args = Args {
        cmd: String::new(),
        config: None,
        model: None,
        positionals: Vec::new(),
        system: None,
        image: None,
        harness: "http://127.0.0.1:8080".to_string(),
        editor: false,
    };
    let mut i = 0;
    // The first non-flag token is the command.
    while i < raw.len() && raw[i].starts_with('-') {
        i += 1;
    }
    if i < raw.len() {
        args.cmd = raw[i].clone();
        i += 1;
    }
    // Remaining tokens: value-taking flags consume the next token; anything
    // else that starts with `--` is an `=`-style or unknown flag; bare tokens
    // are positionals.
    while i < raw.len() {
        let a = &raw[i];
        let next = raw.get(i + 1).cloned();
        match a.as_str() {
            "--config" => {
                args.config = Some(next.ok_or_else(|| "--config needs a value".to_string())?);
                i += 2;
            }
            "--model" => {
                args.model = Some(next.ok_or_else(|| "--model needs a value".to_string())?);
                i += 2;
            }
            "--system" => {
                args.system = Some(next.ok_or_else(|| "--system needs a value".to_string())?);
                i += 2;
            }
            "--image" => {
                args.image = Some(next.ok_or_else(|| "--image needs a value".to_string())?);
                i += 2;
            }
            "--harness" => {
                args.harness = next.ok_or_else(|| "--harness needs a value".to_string())?;
                i += 2;
            }
            _ => {
                if let Some(v) = a.strip_prefix("--config=") {
                    args.config = Some(v.to_string());
                } else if let Some(v) = a.strip_prefix("--model=") {
                    args.model = Some(v.to_string());
                } else if let Some(v) = a.strip_prefix("--image=") {
                    args.image = Some(v.to_string());
                } else if let Some(v) = a.strip_prefix("--harness=") {
                    args.harness = v.to_string();
                } else if let Some(v) = a.strip_prefix("--system=") {
                    args.system = Some(v.to_string());
                } else if a == "--editor" {
                    args.editor = true;
                } else if a == "--help" || a == "-h" {
                    args.cmd = "help".to_string();
                } else {
                    args.positionals.push(a.clone());
                }
                i += 1;
            }
        }
    }
    Ok(args)
}

fn load(
    flag: Option<&str>,
    model_override: Option<&str>,
) -> Result<(ModelConfig, ConfigSource), String> {
    let (mut cfg, src) = resolve_config(flag).map_err(|e: AdapterError| e.to_string())?;
    if let Some(m) = model_override {
        cfg.model = m.to_string();
    }
    Ok((cfg, src))
}

fn provider_label(cfg: &ModelConfig) -> &'static str {
    match &cfg.provider {
        openengine_ai::ProviderConfig::ApiKey { .. } => "deepseek",
        openengine_ai::ProviderConfig::Local { .. } => "llama.cpp",
    }
}

/// Whether an image turn may be attempted. A DeepSeek `ApiKey` model is
/// definitively text-only (hard-block); a `Local` server may host a VLM even
/// when its `is_vision` metadata is missing, so we allow the attempt and let
/// the server reject it if the loaded model truly cannot see.
fn can_attempt_image(cfg: &ModelConfig) -> bool {
    match &cfg.provider {
        openengine_ai::ProviderConfig::Local { .. } => true,
        openengine_ai::ProviderConfig::ApiKey { .. } => {
            openengine_ai::config::vision_supported(cfg)
        }
    }
}

fn main() -> ExitCode {
    // Load a gitignored workspace .env so keys are available without exporting
    // each shell (real env vars always win over the file).
    let _ = openengine_ai::load_dotenv();
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result: Result<(), String> = match a.cmd.as_str() {
        "describe" => cmd_describe(&a),
        "providers" => cmd_providers(),
        "models" => cmd_models(),
        "list" => cmd_list(&a),
        "check" => cmd_check(&a),
        "load" => cmd_load(&a),
        "test" => cmd_test(&a),
        "chat" => cmd_chat(&a),
        "see" => cmd_see(&a),
        "help" | "" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command: {other}\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_describe(a: &Args) -> Result<(), String> {
    let (cfg, src) = load(a.config.as_deref(), a.model.as_deref())?;
    println!("source:     {src}");
    println!("provider:   {}", provider_label(&cfg));
    println!("model:      {}", cfg.model);
    let endpoint = match &cfg.provider {
        openengine_ai::ProviderConfig::ApiKey { endpoint, key_env } => {
            let has_key = std::env::var(key_env).is_ok();
            format!(
                "{endpoint} (key: {} env '{}')",
                if has_key { "set" } else { "MISSING" },
                key_env
            )
        }
        openengine_ai::ProviderConfig::Local { endpoint, key_env } => format!(
            "{endpoint} (key: {})",
            match key_env.as_deref() {
                Some(e) if std::env::var(e).is_ok() => format!("set via env '{e}'"),
                Some(e) => format!("MISSING env '{e}'"),
                None => "none".into(),
            }
        ),
    };
    println!("endpoint:   {endpoint}");
    Ok(())
}

fn cmd_providers() -> Result<(), String> {
    for row in ["deepseek", "llama.cpp", "openai-compat"] {
        println!("{row}");
    }
    Ok(())
}

fn cmd_models() -> Result<(), String> {
    println!("{:<12} {:<22} {:<6} ctx", "kind", "id", "vision");
    for m in CATALOG {
        println!(
            "{:<12} {:<22} {:<6} {}",
            m.provider_kind,
            m.id,
            if m.vision { "yes" } else { "no" },
            m.context_window
        );
    }
    Ok(())
}

/// Discovery: list models the running endpoint actually serves (/v1/models).
fn cmd_list(a: &Args) -> Result<(), String> {
    let (cfg, _src) = load(a.config.as_deref(), a.model.as_deref())?;
    let models = openengine_ai::providers::management::list_models(&cfg)
        .map_err(|e: AdapterError| format!("discovery failed: {e}"))?;
    println!("{:<4} {:<6} model id", "", "state");
    for m in &models {
        println!(
            "{:<4} {:<6} {}",
            if m.loaded { "▶" } else { "" },
            if m.loaded { "loaded" } else { "avail" },
            m.id
        );
    }
    println!("({} models on endpoint)", models.len());
    Ok(())
}

/// Check whether a model id is vision-capable (unsloth check-vision).
fn cmd_check(a: &Args) -> Result<(), String> {
    let (cfg, _src) = load(a.config.as_deref(), a.model.as_deref())?;
    let model = a
        .positionals
        .first()
        .cloned()
        .unwrap_or_else(|| cfg.model.clone());
    let vision = openengine_ai::providers::management::check_vision(&cfg, &model)
        .map_err(|e: AdapterError| format!("vision check failed: {e}"))?;
    println!(
        "{model}: {}",
        if vision {
            "VISION-capable"
        } else {
            "text (is_vision=false)"
        }
    );
    println!(
        "note: metadata may lag — a live image request is the real test (see `openengine-ai see` / `chat --image`)."
    );
    Ok(())
}

/// Load a model on the endpoint (unsloth inference API). Returns vision status.
fn cmd_load(a: &Args) -> Result<(), String> {
    let (cfg, _src) = load(a.config.as_deref(), a.model.as_deref())?;
    let model = a
        .positionals
        .first()
        .cloned()
        .unwrap_or_else(|| cfg.model.clone());
    println!("loading {model} …");
    let vision = openengine_ai::providers::management::load_model(&cfg, &model)
        .map_err(|e: AdapterError| format!("load failed: {e}"))?;
    println!("loaded {model} — vision={vision}");
    Ok(())
}

fn adapter(cfg: &ModelConfig) -> Result<Box<dyn openengine_ai::ModelAdapter>, String> {
    from_config(cfg).map_err(|e: AdapterError| e.to_string())
}

fn complete_text(cfg: &ModelConfig, system: Option<&str>, prompt: &str) -> Result<String, String> {
    let mut turns = Vec::new();
    if let Some(s) = system {
        turns.push(ChatTurn::text("system", s));
    }
    turns.push(ChatTurn::text("user", prompt));
    let a = adapter(cfg)?;
    a.complete(&turns).map_err(|e: AdapterError| e.to_string())
}

fn cmd_test(a: &Args) -> Result<(), String> {
    let (cfg, src) = load(a.config.as_deref(), a.model.as_deref())?;
    println!("testing: {} @ {}", provider_label(&cfg), cfg.model);
    println!("source:  {src}");
    let start = std::time::Instant::now();
    let reply = complete_text(&cfg, None, "Reply with exactly the single word: OK")?;
    let ms = start.elapsed().as_millis();
    println!("reply:   {reply}");
    println!("latency: {ms} ms");
    let pass =
        reply.trim().eq_ignore_ascii_case("ok") || reply.trim().to_uppercase().starts_with("OK");
    println!(
        "result:  {}",
        if pass {
            "PASS"
        } else {
            "UNEXPECTED REPLY (still 200)"
        }
    );
    Ok(())
}

fn cmd_chat(a: &Args) -> Result<(), String> {
    let (cfg, _src) = load(a.config.as_deref(), a.model.as_deref())?;
    let prompt = a.positionals.first().cloned().unwrap_or_default();
    if prompt.is_empty() {
        return Err("chat needs a message".into());
    }
    // Optional image -> multimodal turn. A known-text DeepSeek ApiKey model is
    // hard-blocked; a Local server (llama.cpp/unsloth) may host a VLM even when
    // metadata says no, so we attempt and let the server decide (it returns a
    // typed "does not support vision" if the loaded model truly can't see).
    if let Some(img) = &a.image {
        if !can_attempt_image(&cfg) {
            return Err(format!(
                "model '{}' is a text-only DeepSeek model; cannot send an image. \
                 Point --config at a vision model or a local VLM.",
                cfg.model
            ));
        }
        let bytes = std::fs::read(img).map_err(|e| format!("cannot read image {img}: {e}"))?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mime = if img.ends_with(".png") {
            "image/png"
        } else {
            "image/jpeg"
        }
        .to_string();
        let mut turns = Vec::new();
        if let Some(s) = &a.system {
            turns.push(ChatTurn::text("system", s));
        }
        turns.push(ChatTurn::with_image(
            "user",
            TextImageContent {
                text: prompt.clone(),
                mime,
                base64: b64,
            },
        ));
        let ad = adapter(&cfg)?;
        let reply = ad
            .complete(&turns)
            .map_err(|e: AdapterError| e.to_string())?;
        println!("{reply}");
        return Ok(());
    }
    let reply = complete_text(&cfg, a.system.as_deref(), &prompt)?;
    println!("{reply}");
    Ok(())
}

fn cmd_see(a: &Args) -> Result<(), String> {
    let (cfg, _src) = load(a.config.as_deref(), a.model.as_deref())?;
    // A Local server may host a VLM even if metadata is missing; let the server
    // decide. Only hard-block a known-text DeepSeek ApiKey model.
    if !can_attempt_image(&cfg) {
        return Err(format!(
            "configured model '{}' is a text-only DeepSeek model — /see needs a VLM. \
             Point --config at a vision model or a local VLM endpoint.",
            cfg.model
        ));
    }
    // Image source: the real editor window (--editor, from the saved PNG) or the
    // headless harness /frame.
    let b64: String = if a.editor {
        let file = ".editor/frame.png";
        let bytes = std::fs::read(file).map_err(|e| {
            format!("no editor frame at {file} ({e}). Click '📷 Send to AI' in the editor first.")
        })?;
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    } else {
        // Fetch /frame from a running harness.
        let url = format!("{}/frame?w=512&h=288", a.harness.trim_end_matches('/'));
        let resp = reqwest::blocking::get(&url)
            .map_err(|e| format!("harness unreachable at {url}: {e}"))?;
        if resp.status() != 200 {
            return Err(format!("GET {url} -> http {}", resp.status()));
        }
        let j: serde_json::Value = resp.json().map_err(|e| format!("bad /frame json: {e}"))?;
        j["png_base64"]
            .as_str()
            .ok_or_else(|| format!("no png_base64 in /frame: {j}"))?
            .to_string()
    };
    let prompt = a
        .positionals
        .first()
        .cloned()
        .unwrap_or_else(|| "Describe this 3D scene in one sentence.".to_string());
    let turns = vec![ChatTurn::with_image(
        "user",
        TextImageContent {
            text: prompt,
            mime: "image/png".into(),
            base64: b64,
        },
    )];
    let ad = adapter(&cfg)?;
    let reply = ad
        .complete(&turns)
        .map_err(|e: AdapterError| e.to_string())?;
    println!("{reply}");
    Ok(())
}
