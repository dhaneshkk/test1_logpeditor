//! OpenAI-compatible `/v1/chat/completions` facade over APE.
//!
//! * `model`  — `ape-<output_type>` (e.g. `ape-drspp`, `ape-fol`, `ape-owlfss`, `ape-paraphrase1`);
//!   plain `ape` defaults to `drspp`.
//! * The ACE text is the content of the **last `user` message**.
//! * Optional APE switches go in an `ape` object, e.g. `{"ape": {"guess": "on", "noclex": "off"}}`.
//! * `usage` reports ACE token counts (APE's tokenizer is the ground truth, so prompt_tokens is the
//!   number of tokens APE produced; completion_tokens counts whitespace-separated output pieces).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub ape: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    #[serde(deserialize_with = "content_text")]
    pub content: String,
}

/// Accept both `"content": "text"` and the array-of-parts form `[{"type":"text","text":...}]`.
fn content_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::String(s) => s,
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

/// Map an OpenAI `model` name to an APE output type.
pub fn model_to_output(model: Option<&str>) -> Result<&'static str, String> {
    let m = model.unwrap_or("ape");
    let ty = if m == "ape" { "drspp" } else { m.strip_prefix("ape-").ok_or_else(|| format!("unknown model `{m}`"))? };
    crate::OUTPUT_TYPES
        .iter()
        .copied()
        .find(|t| *t == ty)
        .ok_or_else(|| format!("unknown model `{m}`; use ape-<{}>", crate::OUTPUT_TYPES.join("|")))
}

pub fn models_list() -> Value {
    let data: Vec<Value> = std::iter::once("ape".to_string())
        .chain(crate::OUTPUT_TYPES.iter().map(|t| format!("ape-{t}")))
        .map(|id| json!({"id": id, "object": "model", "created": 0, "owned_by": "attempto"}))
        .collect();
    json!({"object": "list", "data": data})
}

pub fn completion_response(id: &str, model: &str, created: u64, content: &str, prompt_tokens: usize) -> Value {
    let completion_tokens = content.split_whitespace().count();
    json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop",
            "logprobs": null
        }],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens
        }
    })
}

/// One SSE chunk stream (single delta + [DONE]) for clients that insist on `stream: true`.
pub fn stream_body(id: &str, model: &str, created: u64, content: &str) -> String {
    let chunk = json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": model,
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": content}, "finish_reason": null}]});
    let end = json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": model,
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]});
    format!("data: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n")
}

pub fn error_body(msg: &str, kind: &str) -> Value {
    json!({"error": {"message": msg, "type": kind, "param": null, "code": null}})
}
