//! Streaming Server-Sent Events (SSE) and HTTPS LLM client (`SPEC-007-R3`, `SPEC-016-R6`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{AiError, Result};

const ANTHROPIC_API_VERSION: &str = "2023-06-01";
const DEFAULT_ANTHROPIC_MAX_TOKENS: u32 = 2048;

/// Message in a chat completion request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SseMessage {
    pub role: String,
    pub content: String,
}

/// Request payload for an OpenAI-compatible SSE streaming endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseChatRequest {
    pub model: String,
    pub messages: Vec<SseMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
}

/// Incremental event emitted while streaming an LLM response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamEvent {
    Token(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
    },
    Done {
        finish_reason: Option<String>,
    },
}

/// Parses one `data: ...` SSE line from an OpenAI-compatible stream (`SPEC-007-R3`, `SPEC-016-R6`).
///
/// Inspects `delta` content and `tool_calls` prior to `finish_reason` so final-chunk deltas
/// are never dropped. Use [`parse_openai_sse_events`] to receive both events when a single
/// chunk carries both a delta and a `finish_reason`.
#[must_use]
pub fn parse_openai_sse_line(line: &str) -> Option<StreamEvent> {
    parse_openai_sse_events(line).into_iter().next()
}

/// Parses one `data: ...` SSE line from an OpenAI-compatible stream into zero or more events.
#[must_use]
pub fn parse_openai_sse_events(line: &str) -> Vec<StreamEvent> {
    let trimmed = line.trim();
    let Some(data) = trimmed.strip_prefix("data:").map(str::trim) else {
        return Vec::new();
    };
    if data.is_empty() {
        return Vec::new();
    }
    if data == "[DONE]" {
        return vec![StreamEvent::Done {
            finish_reason: Some("stop".to_string()),
        }];
    }
    let Ok(json) = serde_json::from_str::<Value>(data) else {
        return Vec::new();
    };
    let Some(choice) = json
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
    else {
        return Vec::new();
    };

    let mut events = Vec::new();
    if let Some(delta) = choice.get("delta") {
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tc in tool_calls {
                let index = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let id = tc.get("id").and_then(Value::as_str).map(str::to_string);
                let func = tc.get("function");
                let name = func
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let arguments_delta = func
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                events.push(StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                });
            }
        }
        if let Some(content) = delta.get("content").and_then(Value::as_str) {
            if !content.is_empty() {
                events.push(StreamEvent::Token(content.to_string()));
            }
        }
    }

    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        if !reason.is_empty() {
            events.push(StreamEvent::Done {
                finish_reason: Some(reason.to_string()),
            });
        }
    }

    events
}

/// Parses one `data: ...` SSE line from an Anthropic `/v1/messages` stream (`SPEC-016-R6`).
#[must_use]
pub fn parse_anthropic_sse_line(line: &str) -> Option<StreamEvent> {
    let trimmed = line.trim();
    let data = trimmed.strip_prefix("data:")?.trim();
    if data.is_empty() {
        return None;
    }
    let json: Value = serde_json::from_str(data).ok()?;
    let event_type = json.get("type").and_then(Value::as_str)?;
    match event_type {
        "content_block_start" => {
            let index = json.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let block = json.get("content_block")?;
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                Some(StreamEvent::ToolCallDelta {
                    index,
                    id: block.get("id").and_then(Value::as_str).map(str::to_string),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    arguments_delta: String::new(),
                })
            } else {
                None
            }
        }
        "content_block_delta" => {
            let index = json.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let delta = json.get("delta")?;
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => {
                    let text = delta.get("text").and_then(Value::as_str)?;
                    (!text.is_empty()).then(|| StreamEvent::Token(text.to_string()))
                }
                Some("input_json_delta") => {
                    let partial = delta
                        .get("partial_json")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    Some(StreamEvent::ToolCallDelta {
                        index,
                        id: None,
                        name: None,
                        arguments_delta: partial,
                    })
                }
                _ => None,
            }
        }
        "message_delta" => {
            let reason = json
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(Value::as_str)
                .map(str::to_string);
            reason.map(|r| StreamEvent::Done {
                finish_reason: Some(r),
            })
        }
        "message_stop" => Some(StreamEvent::Done {
            finish_reason: Some("end_turn".to_string()),
        }),
        _ => None,
    }
}

/// Streams chat completions from an OpenAI-compatible `/chat/completions` SSE endpoint.
///
/// # Errors
///
/// Returns [`AiError::Provider`] if the HTTP request fails, returns a non-success status,
/// or breaks mid-stream.
pub async fn stream_openai_chat(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    req: &SseChatRequest,
    mut on_event: impl FnMut(StreamEvent),
) -> Result<()> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut body = serde_json::to_value(req).map_err(aitools_core::AitoolsError::Json)?;
    if let Value::Object(ref mut map) = body {
        map.insert("stream".to_string(), Value::Bool(true));
    }

    let mut builder = client.post(url).json(&body);
    if !api_key.is_empty() {
        builder = builder.bearer_auth(api_key);
    }

    let mut resp = builder
        .send()
        .await
        .map_err(|e| AiError::Provider(format!("SSE request failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(AiError::Provider(format!(
            "SSE endpoint returned HTTP {}",
            resp.status()
        )));
    }

    let mut buffer = String::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| AiError::Provider(format!("SSE chunk read failed: {e}")))?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].to_string();
            buffer.drain(..=newline_pos);
            for ev in parse_openai_sse_events(&line) {
                on_event(ev);
            }
        }
    }
    if !buffer.trim().is_empty() {
        for ev in parse_openai_sse_events(&buffer) {
            on_event(ev);
        }
    }

    Ok(())
}

/// Request payload for an Anthropic `/v1/messages` SSE streaming endpoint (`SPEC-016-R6`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicChatRequest {
    /// Upstream Claude model identifier.
    pub model: String,
    /// Optional system prompt string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Conversation messages (`user` / `assistant`).
    pub messages: Vec<SseMessage>,
    /// Optional tool declarations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
}

/// Streams chat completions from an Anthropic `/v1/messages` SSE endpoint (`SPEC-016-R6`).
///
/// # Errors
///
/// Returns [`AiError::Provider`] if the HTTP request fails, returns a non-success status,
/// or breaks mid-stream.
pub async fn stream_anthropic_chat(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    req: &AnthropicChatRequest,
    mut on_event: impl FnMut(StreamEvent),
) -> Result<()> {
    let trimmed = base_url.trim_end_matches('/');
    let url = if trimmed.ends_with("/v1") {
        format!("{trimmed}/messages")
    } else {
        format!("{trimmed}/v1/messages")
    };

    let filtered_messages: Vec<Value> = req
        .messages
        .iter()
        .filter(|m| m.role != "system")
        .map(|m| {
            json!({
                "role": if m.role == "assistant" { "assistant" } else { "user" },
                "content": m.content,
            })
        })
        .collect();

    let mut body = json!({
        "model": req.model,
        "max_tokens": DEFAULT_ANTHROPIC_MAX_TOKENS,
        "messages": filtered_messages,
        "stream": true,
    });
    if let Value::Object(ref mut map) = body {
        if let Some(sys) = req
            .system
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            map.insert("system".to_string(), Value::String(sys.to_string()));
        }
        if !req.tools.is_empty() {
            map.insert("tools".to_string(), Value::Array(req.tools.clone()));
        }
    }

    let mut resp = client
        .post(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_API_VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|e| AiError::Provider(format!("Anthropic SSE request failed: {e}")))?;

    if !resp.status().is_success() {
        return Err(AiError::Provider(format!(
            "Anthropic SSE endpoint returned HTTP {}",
            resp.status()
        )));
    }

    let mut buffer = String::new();
    let mut emitted_done = false;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| AiError::Provider(format!("Anthropic SSE chunk read failed: {e}")))?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].to_string();
            buffer.drain(..=newline_pos);
            if let Some(ev) = parse_anthropic_sse_line(&line) {
                if matches!(ev, StreamEvent::Done { .. }) {
                    if emitted_done {
                        continue;
                    }
                    emitted_done = true;
                }
                on_event(ev);
            }
        }
    }
    if !buffer.trim().is_empty() {
        if let Some(ev) = parse_anthropic_sse_line(&buffer) {
            if !matches!(ev, StreamEvent::Done { .. }) || !emitted_done {
                on_event(ev);
            }
        }
    }

    Ok(())
}

/// Calls an OpenAI-compatible `/embeddings` endpoint (`SPEC-016-R6`).
///
/// # Errors
///
/// Returns [`AiError::Provider`] if the HTTP request fails, returns a non-success status,
/// or yields malformed JSON vectors.
pub async fn fetch_openai_embeddings(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    texts: &[String],
) -> Result<Vec<Vec<f32>>> {
    let url = format!("{}/embeddings", base_url.trim_end_matches('/'));
    let body = json!({
        "model": model,
        "input": texts,
    });
    let mut builder = client.post(url).json(&body);
    if !api_key.is_empty() {
        builder = builder.bearer_auth(api_key);
    }
    let resp = builder
        .send()
        .await
        .map_err(|e| AiError::Provider(format!("embedding request failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(AiError::Provider(format!(
            "embedding endpoint returned HTTP {}",
            resp.status()
        )));
    }
    let json: Value = resp
        .json()
        .await
        .map_err(|e| AiError::Provider(format!("invalid embedding JSON: {e}")))?;
    let data = json
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| AiError::Provider("embedding response missing 'data' array".into()))?;
    let mut vectors = Vec::with_capacity(data.len());
    for item in data {
        let arr = item
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or_else(|| AiError::Provider("embedding entry missing 'embedding' array".into()))?;
        let vec_f32: Vec<f32> = arr
            .iter()
            .filter_map(Value::as_f64)
            .map(|v| v as f32)
            .collect();
        vectors.push(vec_f32);
    }
    Ok(vectors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_007_r3_parses_streaming_sse_tokens_and_tool_calls() {
        let token_line =
            r#"data: {"choices":[{"delta":{"content":"Hello "},"finish_reason":null}]}"#;
        assert_eq!(
            parse_openai_sse_line(token_line),
            Some(StreamEvent::Token("Hello ".into()))
        );

        let tool_line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"os.files.find","arguments":"{\"query\":"}}]},"finish_reason":null}]}"#;
        assert_eq!(
            parse_openai_sse_line(tool_line),
            Some(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                name: Some("os.files.find".into()),
                arguments_delta: "{\"query\":".into(),
            })
        );

        assert_eq!(
            parse_openai_sse_line("data: [DONE]"),
            Some(StreamEvent::Done {
                finish_reason: Some("stop".into()),
            })
        );

        // Final chunk carrying both content and finish_reason must emit Token before Done.
        let combined = r#"data: {"choices":[{"delta":{"content":"!"},"finish_reason":"stop"}]}"#;
        let combined_events = parse_openai_sse_events(combined);
        assert_eq!(
            combined_events,
            vec![
                StreamEvent::Token("!".into()),
                StreamEvent::Done {
                    finish_reason: Some("stop".into())
                }
            ]
        );

        // Anthropic SSE parsing
        let ant_start = r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"os.clipboard.read"}}"#;
        assert_eq!(
            parse_anthropic_sse_line(ant_start),
            Some(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("toolu_1".into()),
                name: Some("os.clipboard.read".into()),
                arguments_delta: String::new(),
            })
        );
        let ant_delta = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#;
        assert_eq!(
            parse_anthropic_sse_line(ant_delta),
            Some(StreamEvent::Token("Hi".into()))
        );
        let ant_stop = r#"data: {"type":"message_stop"}"#;
        assert_eq!(
            parse_anthropic_sse_line(ant_stop),
            Some(StreamEvent::Done {
                finish_reason: Some("end_turn".into())
            })
        );
    }
}
