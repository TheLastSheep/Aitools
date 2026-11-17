//! Streaming Server-Sent Events (SSE) LLM client (`SPEC-007-R3`, Milestone 10).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AiError, Result};

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

/// Parses one `data: ...` SSE line from an OpenAI-compatible stream (`SPEC-007-R3`).
#[must_use]
pub fn parse_openai_sse_line(line: &str) -> Option<StreamEvent> {
    let trimmed = line.trim();
    let data = trimmed.strip_prefix("data:")?.trim();
    if data.is_empty() {
        return None;
    }
    if data == "[DONE]" {
        return Some(StreamEvent::Done {
            finish_reason: Some("stop".to_string()),
        });
    }
    let json: Value = serde_json::from_str(data).ok()?;
    let choice = json.get("choices")?.as_array()?.first()?;

    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        if !reason.is_empty() {
            return Some(StreamEvent::Done {
                finish_reason: Some(reason.to_string()),
            });
        }
    }

    let delta = choice.get("delta")?;
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        if let Some(tc) = tool_calls.first() {
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
            return Some(StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            });
        }
    }

    let content = delta.get("content").and_then(Value::as_str)?;
    if content.is_empty() {
        None
    } else {
        Some(StreamEvent::Token(content.to_string()))
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
            if let Some(ev) = parse_openai_sse_line(&line) {
                on_event(ev);
            }
        }
    }
    if !buffer.trim().is_empty() {
        if let Some(ev) = parse_openai_sse_line(&buffer) {
            on_event(ev);
        }
    }

    Ok(())
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
    }
}
