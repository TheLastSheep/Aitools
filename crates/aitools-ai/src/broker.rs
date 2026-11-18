//! Centralized multi-provider model broker & per-plugin AI quota enforcement (`SPEC-007-R3`, `SPEC-007-R4`, `SPEC-016-R6`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aitools_core::PluginAiQuotas;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::stream::{
    fetch_openai_embeddings, stream_anthropic_chat, stream_openai_chat, AnthropicChatRequest,
    SseChatRequest, SseMessage, StreamEvent,
};
use crate::{AiError, Result};

/// Supported LLM provider backends (`SPEC-007-R3`, `SPEC-016-R6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI `/v1/chat/completions` compatible HTTPS API.
    OpenAiCompatible,
    /// Anthropic `/v1/messages` HTTPS API.
    Anthropic,
    /// Google Gemini API (OpenAI-compatible endpoint or native fallback).
    Gemini,
    /// Local Ollama runtime (no API key required).
    Ollama,
    /// Deterministic local fallback when no live credential is configured.
    OfflineMock,
}

/// Named model routing profile (`SPEC-007-R3`, `SPEC-016-R6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProfileKind {
    /// Low-latency everyday tasks (`"fast"`).
    Fast,
    /// Balanced quality/latency profile (`"balanced"`).
    Balanced,
    /// Complex multi-step reasoning (`"deep"`).
    Deep,
    /// Legacy alias for deep reasoning (`"reasoning"`).
    Reasoning,
    /// Vector embedding model (`"embedding"`).
    Embedding,
}

impl ModelProfileKind {
    /// Parses a profile label leniently, defaulting unknown values to [`Self::Balanced`].
    #[must_use]
    pub fn parse_lenient(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "fast" => Self::Fast,
            "deep" => Self::Deep,
            "reasoning" => Self::Reasoning,
            "embedding" | "embed" => Self::Embedding,
            _ => Self::Balanced,
        }
    }

    /// Canonical lowercase profile slug.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Deep => "deep",
            Self::Reasoning => "reasoning",
            Self::Embedding => "embedding",
        }
    }
}

/// Configured model profile pointing to an endpoint and OS Keychain account (`INV-SEC-03`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Logical profile tier.
    pub kind: ModelProfileKind,
    /// Upstream provider protocol.
    pub provider: ProviderKind,
    /// Base API URL (e.g. `https://api.openai.com/v1` or `https://api.anthropic.com`).
    pub base_url: String,
    /// Upstream model identifier (e.g. `gpt-4o-mini`, `claude-3-5-sonnet-latest`).
    pub model: String,
    /// Keychain account name storing the secret API key (`INV-SEC-03`).
    pub keychain_account: String,
}

/// Backend mode that serviced an AI invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackend {
    /// Executed against a live configured HTTPS/local LLM provider.
    LiveProvider,
    /// Executed using deterministic offline mock fallback because no key/profile was configured.
    OfflineMock,
}

/// Outcome of a buffered text completion call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatOutcome {
    /// Accumulated completion text.
    pub content: String,
    /// Whether a live provider or offline fallback handled the request.
    pub backend: ExecutionBackend,
    /// Model identifier that generated the response.
    pub model: String,
}

/// Outcome of an embedding batch request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbedOutcome {
    /// Dense float embedding vectors aligned with input texts.
    pub vectors: Vec<Vec<f32>>,
    /// Whether a live provider or deterministic offline hash vectors answered the request.
    pub backend: ExecutionBackend,
    /// Model identifier that generated the embeddings.
    pub model: String,
}

/// Abstracts OS Keychain API key lookup without violating `SPEC-000` crate DAG boundaries.
pub trait ApiKeyProvider: Send + Sync {
    /// Resolves a secret API key by Keychain `account`, returning `None` if absent.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::Provider`] if reading from the credential store fails.
    fn get_api_key(&self, account: &str) -> Result<Option<String>>;
}

/// Always reports no configured API keys, triggering clean [`ExecutionBackend::OfflineMock`] fallback.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopApiKeyProvider;

impl ApiKeyProvider for NoopApiKeyProvider {
    fn get_api_key(&self, _account: &str) -> Result<Option<String>> {
        Ok(None)
    }
}

/// Deterministic in-memory key provider for unit and integration tests.
#[derive(Debug, Default)]
pub struct MemoryApiKeyProvider {
    keys: Mutex<HashMap<String, String>>,
}

impl MemoryApiKeyProvider {
    /// Inserts an API key under `account` for testing.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn insert(&self, account: impl Into<String>, secret: impl Into<String>) {
        self.keys
            .lock()
            .expect("memory api key mutex poisoned")
            .insert(account.into(), secret.into());
    }
}

impl ApiKeyProvider for MemoryApiKeyProvider {
    fn get_api_key(&self, account: &str) -> Result<Option<String>> {
        Ok(self
            .keys
            .lock()
            .expect("memory api key mutex poisoned")
            .get(account)
            .cloned())
    }
}

#[derive(Debug, Default)]
struct PluginUsage {
    window_start: Option<Instant>,
    requests_in_window: u32,
    tokens_today: u64,
}

/// Centralized model & quota broker (`SPEC-007-R3`, `SPEC-007-R4`, `SPEC-016-R6`).
#[derive(Debug, Clone, Default)]
pub struct ModelBroker {
    profiles: HashMap<ModelProfileKind, ModelProfile>,
    quotas: HashMap<String, PluginAiQuotas>,
    usage: std::sync::Arc<Mutex<HashMap<String, PluginUsage>>>,
}

impl ModelBroker {
    /// Creates an empty [`ModelBroker`] with no pre-registered profiles.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or replaces a [`ModelProfile`].
    pub fn register_profile(&mut self, profile: ModelProfile) {
        self.profiles.insert(profile.kind, profile);
    }

    /// Lists all configured profiles in deterministic order (for `allAiModels()`).
    #[must_use]
    pub fn list_profiles(&self) -> Vec<ModelProfile> {
        let order = [
            ModelProfileKind::Fast,
            ModelProfileKind::Balanced,
            ModelProfileKind::Deep,
            ModelProfileKind::Reasoning,
            ModelProfileKind::Embedding,
        ];
        order
            .into_iter()
            .filter_map(|k| self.profiles.get(&k).cloned())
            .collect()
    }

    /// Returns `true` when `keys` holds a non-empty credential for `api_key_ref`.
    #[must_use]
    pub fn has_configured_key(&self, keys: &dyn ApiKeyProvider, api_key_ref: &str) -> bool {
        let _ = self;
        keys.get_api_key(api_key_ref)
            .ok()
            .flatten()
            .is_some_and(|k| !k.trim().is_empty())
    }

    /// Resolves the exact [`ModelProfile`] registered for `kind`.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::Provider`] if no profile has been registered for `kind`.
    pub fn resolve_profile(&self, kind: ModelProfileKind) -> Result<&ModelProfile> {
        self.profiles
            .get(&kind)
            .ok_or_else(|| AiError::Provider(format!("no model profile configured for {kind:?}")))
    }

    /// Resolves `kind` with sensible tier fallbacks (`Balanced -> Fast`, `Deep <-> Reasoning -> Balanced -> Fast`).
    #[must_use]
    pub fn resolve_profile_with_fallback(&self, kind: ModelProfileKind) -> Option<&ModelProfile> {
        if let Some(p) = self.profiles.get(&kind) {
            return Some(p);
        }
        let fallbacks: &[ModelProfileKind] = match kind {
            ModelProfileKind::Fast => &[ModelProfileKind::Balanced],
            ModelProfileKind::Balanced => &[ModelProfileKind::Fast, ModelProfileKind::Deep],
            ModelProfileKind::Deep => &[
                ModelProfileKind::Reasoning,
                ModelProfileKind::Balanced,
                ModelProfileKind::Fast,
            ],
            ModelProfileKind::Reasoning => &[
                ModelProfileKind::Deep,
                ModelProfileKind::Balanced,
                ModelProfileKind::Fast,
            ],
            ModelProfileKind::Embedding => &[],
        };
        fallbacks
            .iter()
            .find_map(|candidate| self.profiles.get(candidate))
    }

    /// Configures rate and token limits for `plugin_id`.
    pub fn set_plugin_quota(&mut self, plugin_id: impl Into<String>, quota: PluginAiQuotas) {
        self.quotas.insert(plugin_id.into(), quota);
    }

    /// Checks and consumes per-plugin rate limit and token quota (`SPEC-007-R4`).
    ///
    /// # Errors
    ///
    /// Returns [`AiError::QuotaExceeded`] if the plugin exceeds its configured requests-per-minute
    /// or daily token budget.
    ///
    /// # Panics
    ///
    /// Panics if the internal usage tracking mutex is poisoned.
    pub fn check_and_consume_quota(
        &self,
        plugin_id: &str,
        estimated_tokens: u64,
        now: Instant,
    ) -> Result<()> {
        let Some(limit) = self.quotas.get(plugin_id) else {
            return Ok(());
        };
        let mut usage_map = self
            .usage
            .lock()
            .expect("model broker usage mutex poisoned");
        let entry = usage_map.entry(plugin_id.to_string()).or_default();

        if entry
            .window_start
            .is_none_or(|start| now.duration_since(start) >= Duration::from_secs(60))
        {
            entry.window_start = Some(now);
            entry.requests_in_window = 0;
        }

        if entry.requests_in_window >= limit.requests_per_minute {
            return Err(AiError::QuotaExceeded {
                plugin_id: plugin_id.to_string(),
                reason: format!(
                    "rate limit exceeded ({} req/min)",
                    limit.requests_per_minute
                ),
            });
        }
        if entry.tokens_today + estimated_tokens > limit.tokens_per_day {
            return Err(AiError::QuotaExceeded {
                plugin_id: plugin_id.to_string(),
                reason: format!("daily token quota exceeded ({})", limit.tokens_per_day),
            });
        }

        entry.requests_in_window += 1;
        entry.tokens_today += estimated_tokens;
        Ok(())
    }

    /// Resolves the active profile and OS Keychain credential for `kind` without logging any secret.
    fn resolve_live_target(
        &self,
        kind: ModelProfileKind,
        keys: &dyn ApiKeyProvider,
    ) -> Option<(ModelProfile, String)> {
        let profile = self.resolve_profile_with_fallback(kind)?.clone();
        match profile.provider {
            ProviderKind::OfflineMock => None,
            ProviderKind::Ollama => {
                let key = keys
                    .get_api_key(&profile.keychain_account)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                Some((profile, key))
            }
            ProviderKind::OpenAiCompatible | ProviderKind::Anthropic | ProviderKind::Gemini => {
                let key = keys
                    .get_api_key(&profile.keychain_account)
                    .ok()
                    .flatten()
                    .map(|k| k.trim().to_string())
                    .filter(|k| !k.is_empty())?;
                Some((profile, key))
            }
        }
    }

    /// Streams chat tokens and tool-call deltas using configured Keychain HTTPS/SSE providers,
    /// falling back cleanly to [`ExecutionBackend::OfflineMock`] when unconfigured (`SPEC-016-R6`).
    ///
    /// # Errors
    ///
    /// Returns [`AiError::QuotaExceeded`] if plugin quotas are exceeded, or [`AiError::Provider`]
    /// if a configured live provider fails over HTTPS.
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit parameters preserve trait-object flexibility for ApiKeyProvider and SSE callback"
    )]
    pub async fn stream_chat(
        &self,
        client: &reqwest::Client,
        keys: &dyn ApiKeyProvider,
        plugin_id: &str,
        kind: ModelProfileKind,
        system: Option<&str>,
        messages: &[SseMessage],
        tools: &[Value],
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<ChatOutcome> {
        let est_chars: usize =
            system.map_or(0, str::len) + messages.iter().map(|m| m.content.len()).sum::<usize>();
        let est_tokens = ((est_chars / 4) as u64).max(1);
        self.check_and_consume_quota(plugin_id, est_tokens, Instant::now())?;

        let Some((profile, api_key)) = self.resolve_live_target(kind, keys) else {
            let prompt_preview = messages.last().map_or("", |m| m.content.as_str());
            let reply = offline_mock_reply(plugin_id, kind, prompt_preview);
            for chunk in chunk_words(&reply) {
                on_event(StreamEvent::Token(chunk));
            }
            on_event(StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            });
            return Ok(ChatOutcome {
                content: reply,
                backend: ExecutionBackend::OfflineMock,
                model: format!("offline-mock:{}", kind.as_str()),
            });
        };

        let mut full_text = String::new();
        match profile.provider {
            ProviderKind::Anthropic => {
                let req = AnthropicChatRequest {
                    model: profile.model.clone(),
                    system: system.map(str::to_string),
                    messages: messages.to_vec(),
                    tools: tools.to_vec(),
                };
                stream_anthropic_chat(client, &profile.base_url, &api_key, &req, |ev| {
                    if let StreamEvent::Token(ref delta) = ev {
                        full_text.push_str(delta);
                    }
                    on_event(ev);
                })
                .await?;
            }
            ProviderKind::OpenAiCompatible | ProviderKind::Ollama | ProviderKind::Gemini => {
                let mut full_messages = Vec::with_capacity(messages.len() + 1);
                if let Some(sys) = system.map(str::trim).filter(|s| !s.is_empty()) {
                    full_messages.push(SseMessage {
                        role: "system".to_string(),
                        content: sys.to_string(),
                    });
                }
                full_messages.extend(messages.iter().cloned());
                let req = SseChatRequest {
                    model: profile.model.clone(),
                    messages: full_messages,
                    tools: tools.to_vec(),
                };
                stream_openai_chat(client, &profile.base_url, &api_key, &req, |ev| {
                    if let StreamEvent::Token(ref delta) = ev {
                        full_text.push_str(delta);
                    }
                    on_event(ev);
                })
                .await?;
            }
            ProviderKind::OfflineMock => {}
        }

        Ok(ChatOutcome {
            content: full_text,
            backend: ExecutionBackend::LiveProvider,
            model: profile.model,
        })
    }

    /// Executes a buffered chat completion request (`SPEC-016-R6`).
    ///
    /// # Errors
    ///
    /// Returns [`AiError::QuotaExceeded`] or [`AiError::Provider`] on quota or transport failure.
    #[expect(
        clippy::too_many_arguments,
        reason = "Matches SDK chat request fields cleanly without intermediate wrapper allocations"
    )]
    pub async fn chat_text(
        &self,
        client: &reqwest::Client,
        keys: &dyn ApiKeyProvider,
        plugin_id: &str,
        kind: ModelProfileKind,
        system: Option<&str>,
        prompt: &str,
        schema: Option<&Value>,
    ) -> Result<ChatOutcome> {
        let effective_system = match (system, schema) {
            (Some(sys), Some(sch)) => Some(format!(
                "{sys}\nRespond strictly with valid JSON matching schema: {sch}"
            )),
            (None, Some(sch)) => Some(format!(
                "Respond strictly with valid JSON matching schema: {sch}"
            )),
            (Some(sys), None) => Some(sys.to_string()),
            (None, None) => None,
        };
        let messages = [SseMessage {
            role: "user".to_string(),
            content: prompt.to_string(),
        }];
        self.stream_chat(
            client,
            keys,
            plugin_id,
            kind,
            effective_system.as_deref(),
            &messages,
            &[],
            |_| {},
        )
        .await
    }

    /// Generates normalized vector embeddings via a configured live provider or deterministic
    /// 64-dimensional offline hash embeddings when unconfigured (`SPEC-016-R6`).
    ///
    /// # Errors
    ///
    /// Returns [`AiError::QuotaExceeded`] or [`AiError::Provider`] on quota or transport failure.
    pub async fn embed_texts(
        &self,
        client: &reqwest::Client,
        keys: &dyn ApiKeyProvider,
        plugin_id: &str,
        texts: &[String],
    ) -> Result<EmbedOutcome> {
        let total_chars: usize = texts.iter().map(String::len).sum();
        let est_tokens = ((total_chars / 4) as u64).max(1);
        self.check_and_consume_quota(plugin_id, est_tokens, Instant::now())?;

        let Some((profile, api_key)) = self.resolve_live_target(ModelProfileKind::Embedding, keys)
        else {
            let vectors = texts
                .iter()
                .map(|t| deterministic_hash_embedding(t))
                .collect();
            return Ok(EmbedOutcome {
                vectors,
                backend: ExecutionBackend::OfflineMock,
                model: "offline-mock:embedding".to_string(),
            });
        };

        let vectors =
            fetch_openai_embeddings(client, &profile.base_url, &api_key, &profile.model, texts)
                .await?;
        Ok(EmbedOutcome {
            vectors,
            backend: ExecutionBackend::LiveProvider,
            model: profile.model,
        })
    }
}

fn offline_mock_reply(plugin_id: &str, kind: ModelProfileKind, prompt: &str) -> String {
    let preview: String = prompt.trim().chars().take(120).collect();
    format!(
        "[offline-mock:{}] No live API key configured in OS Keychain for plugin '{}'. Received ({} chars): {}",
        kind.as_str(),
        plugin_id,
        prompt.len(),
        preview
    )
}

fn chunk_words(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        if ch.is_whitespace() && idx > start {
            chunks.push(text[start..=idx].to_string());
            start = idx + ch.len_utf8();
        }
    }
    if start < text.len() {
        chunks.push(text[start..].to_string());
    }
    if chunks.is_empty() && !text.is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}

fn deterministic_hash_embedding(text: &str) -> Vec<f32> {
    const DIMS: usize = 64;
    let mut vec = vec![0.0f32; DIMS];
    if text.is_empty() {
        vec[0] = 1.0;
        return vec;
    }
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let bucket = (usize::from(b).wrapping_mul(31).wrapping_add(i * 7)) % DIMS;
        let sign = if (b ^ (i as u8)) & 1 == 0 { 1.0 } else { -1.0 };
        vec[bucket] += sign;
    }
    let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    }
    vec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_007_r3_and_r4_routes_profiles_and_enforces_quotas() {
        let mut broker = ModelBroker::new();
        broker.register_profile(ModelProfile {
            kind: ModelProfileKind::Fast,
            provider: ProviderKind::OpenAiCompatible,
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4o-mini".into(),
            keychain_account: "openai.fast".into(),
        });
        broker.register_profile(ModelProfile {
            kind: ModelProfileKind::Embedding,
            provider: ProviderKind::Ollama,
            base_url: "http://127.0.0.1:11434".into(),
            model: "nomic-embed-text".into(),
            keychain_account: "ollama.local".into(),
        });
        assert_eq!(
            broker
                .resolve_profile(ModelProfileKind::Fast)
                .unwrap()
                .model,
            "gpt-4o-mini"
        );

        broker.set_plugin_quota(
            "demo",
            PluginAiQuotas {
                requests_per_minute: 2,
                tokens_per_day: 1000,
            },
        );
        let t0 = Instant::now();
        broker.check_and_consume_quota("demo", 400, t0).unwrap();
        broker.check_and_consume_quota("demo", 400, t0).unwrap();
        let err = broker.check_and_consume_quota("demo", 100, t0).unwrap_err();
        assert!(matches!(err, AiError::QuotaExceeded { .. }));
    }

    #[tokio::test]
    async fn spec_016_r6_broker_streams_live_sse_and_falls_back_when_unconfigured() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let broker = ModelBroker::new();
        let client = reqwest::Client::new();
        let noop_keys = NoopApiKeyProvider;

        // Without configured profiles/keys -> clean OfflineMock fallback.
        let offline = broker
            .chat_text(
                &client,
                &noop_keys,
                "translation",
                ModelProfileKind::Balanced,
                None,
                "Hello world",
                None,
            )
            .await
            .unwrap();
        assert_eq!(offline.backend, ExecutionBackend::OfflineMock);
        assert!(offline.content.contains("offline-mock:balanced"));

        let embed_offline = broker
            .embed_texts(
                &client,
                &noop_keys,
                "translation",
                &["hello".to_string(), "world".to_string()],
            )
            .await
            .unwrap();
        assert_eq!(embed_offline.backend, ExecutionBackend::OfflineMock);
        assert_eq!(embed_offline.vectors.len(), 2);
        assert_eq!(embed_offline.vectors[0].len(), 64);

        // Live local SSE mock server over TCP loopback with Keychain key lookup.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let sse_body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Live \"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"SSE!\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse_body.len(),
                sse_body
            );
            stream.write_all(resp.as_bytes()).unwrap();
        });

        let mut live_broker = ModelBroker::new();
        live_broker.register_profile(ModelProfile {
            kind: ModelProfileKind::Balanced,
            provider: ProviderKind::OpenAiCompatible,
            base_url: format!("http://{addr}/v1"),
            model: "gpt-4o".into(),
            keychain_account: "openai.balanced".into(),
        });
        let keys = MemoryApiKeyProvider::default();
        keys.insert("openai.balanced", "sk-secret-test");

        let mut tokens = Vec::new();
        let outcome = live_broker
            .stream_chat(
                &client,
                &keys,
                "translation",
                ModelProfileKind::Balanced,
                Some("Be concise"),
                &[SseMessage {
                    role: "user".into(),
                    content: "Hi".into(),
                }],
                &[],
                |ev| {
                    if let StreamEvent::Token(t) = ev {
                        tokens.push(t);
                    }
                },
            )
            .await
            .unwrap();
        handle.join().unwrap();

        assert_eq!(outcome.backend, ExecutionBackend::LiveProvider);
        assert_eq!(outcome.content, "Live SSE!");
        assert_eq!(tokens, vec!["Live ".to_string(), "SSE!".to_string()]);
    }
}
