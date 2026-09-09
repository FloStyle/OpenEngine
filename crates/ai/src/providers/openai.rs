//! Shared OpenAI-compatible `chat/completions` transport used by the DeepSeek
//! and local llama.cpp adapters. Both speak the same wire protocol; the only
//! difference between providers is auth, which the caller supplies.

use crate::{AdapterError, ChatTurn};

/// Request/response envelope for the OpenAI-compatible chat endpoint.
#[derive(serde::Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatTurn],
}

#[derive(serde::Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(serde::Deserialize)]
struct Choice {
    message: Message,
}

#[derive(serde::Deserialize)]
struct Message {
    content: Option<String>,
}

/// POST one conversation to an OpenAI-compatible `{endpoint}/chat/completions`.
///
/// `endpoint` is the provider base URL that already ends in `/v1` (or wherever
/// `chat/completions` lives). `bearer` is the optional `Authorization` token
/// for API-key providers. Returns the assistant text, or a typed error.
pub(crate) fn chat(
    endpoint: &str,
    bearer: Option<&str>,
    model: &str,
    turns: &[ChatTurn],
) -> Result<String, AdapterError> {
    let url = format!("{endpoint}/chat/completions");
    let req = ChatRequest {
        model,
        messages: turns,
    };

    let client = reqwest::blocking::Client::new();
    let mut rb = client.post(&url).json(&req);
    if let Some(token) = bearer {
        rb = rb.bearer_auth(token);
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
    let parsed: ChatResponse = resp
        .json()
        .map_err(|e| AdapterError::Transport(format!("bad response: {e}")))?;
    parsed
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .ok_or_else(|| AdapterError::Transport("empty assistant reply".into()))
}
