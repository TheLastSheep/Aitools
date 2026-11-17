//! Centralized multi-provider model broker & per-plugin AI quota enforcement (`SPEC-007-R3`, `SPEC-007-R4`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aitools_core::PluginAiQuotas;
use serde::{Deserialize, Serialize};

use crate::{AiError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenAiCompatible,
    Anthropic,
    Gemini,
    Ollama,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProfileKind {
    Fast,
    Reasoning,
    Embedding,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    pub kind: ModelProfileKind,
    pub provider: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub keychain_account: String,
}

#[derive(Debug, Default)]
struct PluginUsage {
    window_start: Option<Instant>,
    requests_in_window: u32,
    tokens_today: u64,
}

/// Centralized model & quota broker (`SPEC-007-R3`, `SPEC-007-R4`).
#[derive(Debug, Default)]
pub struct ModelBroker {
    profiles: HashMap<ModelProfileKind, ModelProfile>,
    quotas: HashMap<String, PluginAiQuotas>,
    usage: Mutex<HashMap<String, PluginUsage>>,
}

impl ModelBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_profile(&mut self, profile: ModelProfile) {
        self.profiles.insert(profile.kind, profile);
    }

    /// Resolves the configured [`ModelProfile`] for `kind`.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::Provider`] if no profile has been registered for `kind`.
    pub fn resolve_profile(&self, kind: ModelProfileKind) -> Result<&ModelProfile> {
        self.profiles
            .get(&kind)
            .ok_or_else(|| AiError::Provider(format!("no model profile configured for {kind:?}")))
    }

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
}
