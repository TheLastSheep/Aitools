//! Unified cross-tier `ToolRegistry` and local cosine-similarity Top-K routing (`SPEC-007-R5`, `SPEC-007-R6`).

use std::collections::{BTreeMap, HashMap};

use aitools_core::{Cmd, ManifestV2, PluginManifest, RiskTier};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::harness::canonicalize_json;

pub const SEMANTIC_FILTER_THRESHOLD: usize = 12;
pub const SEMANTIC_TOP_K: usize = 10;

/// Origin tier of a registered AI tool (`SPEC-007-R5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSource {
    BuiltinOs,
    NativeV2Plugin,
    RustWasmWit,
    LegacyZToolsSynthesized,
    ExternalMcp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredTool {
    pub name: String,
    pub plugin_id: Option<String>,
    pub source: ToolSource,
    pub risk_tier: RiskTier,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_markdown: Option<String>,
}

/// Aggregates tools from all 5 tiers and performs Top-K semantic pre-filtering (`SPEC-007-R6`).
#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, RegisteredTool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, mut tool: RegisteredTool) {
        tool.input_schema = canonicalize_json(&tool.input_schema);
        self.tools.insert(tool.name.clone(), tool);
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&RegisteredTool> {
        self.tools.get(name)
    }

    /// Registers built-in OS capabilities (`SPEC-007-R5` item 1).
    pub fn register_builtin_os_defaults(&mut self) {
        let builtins = [
            (
                "os.clipboard.read",
                RiskTier::Tier0ReadOnly,
                "Read current text from the system clipboard",
                json!({ "type": "object", "properties": {} }),
            ),
            (
                "os.clipboard.write",
                RiskTier::Tier1StateMutating,
                "Write text to the system clipboard",
                json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }),
            ),
            (
                "os.files.find",
                RiskTier::Tier0ReadOnly,
                "Search local files by name or path",
                json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }),
            ),
            (
                "os.apps.launch",
                RiskTier::Tier1StateMutating,
                "Launch an installed desktop application",
                json!({ "type": "object", "properties": { "app": { "type": "string" } }, "required": ["app"] }),
            ),
        ];
        for (name, tier, desc, schema) in builtins {
            self.register(RegisteredTool {
                name: name.to_string(),
                plugin_id: None,
                source: ToolSource::BuiltinOs,
                risk_tier: tier,
                description: desc.to_string(),
                input_schema: schema,
                skill_markdown: None,
            });
        }
    }

    /// Registers tools declared in a v2 `aitools.manifest.json` (`SPEC-007-R5` item 2).
    pub fn register_native_v2(&mut self, manifest: &ManifestV2, skill_markdown: Option<String>) {
        for decl in &manifest.ai_tools {
            self.register(RegisteredTool {
                name: format!("{}.{}", manifest.id, decl.name),
                plugin_id: Some(manifest.id.clone()),
                source: ToolSource::NativeV2Plugin,
                risk_tier: decl.risk_tier,
                description: decl.description.clone(),
                input_schema: decl.input_schema.clone(),
                skill_markdown: skill_markdown.clone(),
            });
        }
    }

    /// Registers tools exported by a `rust-wasm` WIT `agent-tool-provider` (`SPEC-007-R5` item 3).
    pub fn register_wasm_tool(
        &mut self,
        plugin_id: &str,
        tool_name: &str,
        description: &str,
        risk_tier: RiskTier,
        input_schema: Value,
    ) {
        self.register(RegisteredTool {
            name: format!("{plugin_id}.{tool_name}"),
            plugin_id: Some(plugin_id.to_string()),
            source: ToolSource::RustWasmWit,
            risk_tier,
            description: description.to_string(),
            input_schema,
            skill_markdown: None,
        });
    }

    /// Synthesizes tools from legacy ZTools `features[].cmds` + `explain` (`SPEC-007-R5` item 4).
    pub fn register_legacy_ztools(&mut self, plugin_id: &str, manifest: &PluginManifest) {
        for feature in &manifest.features {
            let keywords: Vec<String> = feature
                .cmds
                .iter()
                .filter_map(|cmd| match cmd {
                    Cmd::Text(t) => Some(t.clone()),
                    Cmd::Typed(typed) => typed.label.clone(),
                })
                .collect();
            let desc = if keywords.is_empty() {
                feature.explain.clone()
            } else {
                format!("{} (keywords: {})", feature.explain, keywords.join(", "))
            };
            self.register(RegisteredTool {
                name: format!("{plugin_id}.{}", feature.code),
                plugin_id: Some(plugin_id.to_string()),
                source: ToolSource::LegacyZToolsSynthesized,
                risk_tier: RiskTier::Tier1StateMutating,
                description: desc,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "payload": { "type": "string", "description": "Input text or file path" }
                    }
                }),
                skill_markdown: None,
            });
        }
    }

    /// Registers an external MCP server tool (`SPEC-007-R5` item 5, `SPEC-009-R2` default Tier 2).
    pub fn register_mcp_tool(
        &mut self,
        server_name: &str,
        tool_name: &str,
        description: &str,
        input_schema: Value,
        risk_tier_override: Option<RiskTier>,
    ) {
        self.register(RegisteredTool {
            name: format!("mcp.{server_name}.{tool_name}"),
            plugin_id: Some(server_name.to_string()),
            source: ToolSource::ExternalMcp,
            risk_tier: risk_tier_override.unwrap_or(RiskTier::Tier2DestructiveOrExternal),
            description: description.to_string(),
            input_schema,
            skill_markdown: None,
        });
    }

    /// Selects tools for an LLM turn (`SPEC-007-R6`):
    /// - If `<= 12` tools total, returns all tools.
    /// - If `@<plugin>` is mentioned, includes all tools of that plugin + top semantic hits up to 10.
    /// - Otherwise ranks all tools by local character-trigram cosine similarity and returns top 10.
    pub fn select_for_prompt(&self, prompt: &str, context_summary: &str) -> Vec<&RegisteredTool> {
        let all: Vec<&RegisteredTool> = self.tools.values().collect();
        if all.len() <= SEMANTIC_FILTER_THRESHOLD {
            return all;
        }

        let mentioned_plugins: Vec<String> = prompt
            .split_whitespace()
            .filter_map(|w| w.strip_prefix('@'))
            .map(|p| p.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_'))
            .filter(|p| !p.is_empty())
            .map(|p| p.to_ascii_lowercase())
            .collect();

        let query_text = format!("{prompt} {context_summary}");
        let query_vec = trigram_tf(&query_text);

        let mut scored: Vec<(bool, f64, &RegisteredTool)> = all
            .into_iter()
            .map(|tool| {
                let is_mentioned = tool.plugin_id.as_ref().is_some_and(|pid| {
                    let pid_lower = pid.to_ascii_lowercase();
                    mentioned_plugins.iter().any(|m| m == &pid_lower)
                });
                let doc_text = format!("{} {}", tool.name, tool.description);
                let sim = cosine_similarity(&query_vec, &trigram_tf(&doc_text));
                (is_mentioned, sim, tool)
            })
            .collect();

        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.total_cmp(&a.1))
                .then_with(|| a.2.name.cmp(&b.2.name))
        });

        scored
            .into_iter()
            .take(SEMANTIC_TOP_K)
            .map(|(_, _, tool)| tool)
            .collect()
    }
}

fn trigram_tf(input: &str) -> HashMap<String, f64> {
    let normalized: Vec<char> = input
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let mut tf = HashMap::new();
    if normalized.len() < 3 {
        for word in input.split_whitespace() {
            *tf.entry(word.to_lowercase()).or_insert(0.0) += 1.0;
        }
        return tf;
    }
    for win in normalized.windows(3) {
        let key: String = win.iter().collect();
        *tf.entry(key).or_insert(0.0) += 1.0;
    }
    for word in input.split_whitespace() {
        *tf.entry(format!("w:{}", word.to_lowercase()))
            .or_insert(0.0) += 2.0;
    }
    tf
}

fn cosine_similarity(a: &HashMap<String, f64>, b: &HashMap<String, f64>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let dot: f64 = a
        .iter()
        .filter_map(|(k, v)| b.get(k).map(|bv| v * bv))
        .sum();
    let norm_a: f64 = a.values().map(|v| v * v).sum::<f64>().sqrt();
    let norm_b: f64 = b.values().map(|v| v * v).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aitools_core::{Cmd, Feature};

    #[test]
    fn spec_007_r5_and_r6_unifies_tiers_and_filters_top_10() {
        let mut registry = ToolRegistry::new();
        registry.register_builtin_os_defaults();
        registry.register_wasm_tool(
            "hasher",
            "sha256",
            "Compute cryptographic SHA-256 hash",
            RiskTier::Tier0ReadOnly,
            json!({}),
        );
        registry.register_mcp_tool(
            "github",
            "create_issue",
            "Create GitHub issue",
            json!({}),
            None,
        );
        assert_eq!(
            registry.get("mcp.github.create_issue").unwrap().risk_tier,
            RiskTier::Tier2DestructiveOrExternal
        );

        let legacy = PluginManifest {
            platform: vec![],
            name: "legacy-json".into(),
            title: None,
            version: "1.0.0".into(),
            main: Some("index.html".into()),
            logo: None,
            preload: None,
            description: None,
            author: None,
            homepage: None,
            plugin_setting: None,
            features: vec![Feature {
                platform: vec![],
                code: "format".into(),
                label: Some("Format JSON".into()),
                explain: "Format and pretty-print JSON payloads".into(),
                icon: None,
                main_push: false,
                main_hide: false,
                cmds: vec![Cmd::Text("json".into())],
            }],
            development: None,
            tools: HashMap::new(),
            providers: HashMap::new(),
            permissions: vec![],
        };
        registry.register_legacy_ztools("legacy-json", &legacy);

        // Add enough extra tools to exceed SEMANTIC_FILTER_THRESHOLD (12).
        for i in 0..10 {
            registry.register_wasm_tool(
                &format!("noise-{i}"),
                "op",
                "Unrelated calendar utility operation",
                RiskTier::Tier0ReadOnly,
                json!({}),
            );
        }
        assert!(registry.len() > SEMANTIC_FILTER_THRESHOLD);

        let selected = registry.select_for_prompt("format pretty json payload", "");
        assert_eq!(selected.len(), SEMANTIC_TOP_K);
        assert!(selected.iter().any(|t| t.name == "legacy-json.format"));

        // Explicit @mention guarantees inclusion.
        let mentioned = registry.select_for_prompt("@hasher do something", "");
        assert_eq!(mentioned.len(), SEMANTIC_TOP_K);
        assert!(mentioned.iter().any(|t| t.name == "hasher.sha256"));
    }
}
