//! Unified plugin manifests (ZTools/uTools v1 & `aitools` v2), features, and documents (`SPEC-003`, `SPEC-008`).

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{AitoolsError, Result};

/// Three-tier runtime classification (`SPEC-008`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginRuntimeTier {
    /// Tier 1: Legacy `plugin.json` (v1) webview + `aitools-sync://` + lazy QuickJS.
    LegacyCompat,
    /// Tier 2: Native `aitools.manifest.json` (v2) webview + async Tauri IPC.
    NativeUi,
    /// Tier 3: Sandboxed `wasm32-wasip2` (`wasmtime`) component.
    RustWasm,
    /// Signed native `cdylib` plugin.
    RustNative,
    /// Combined UI + Wasm backend (`aitools.manifest.json` v2).
    Hybrid,
}

/// Risk tier for HITL safety enforcement (`SPEC-007-R7`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RiskTier {
    #[default]
    Tier0ReadOnly,
    Tier1StateMutating,
    Tier2DestructiveOrExternal,
}

/// Native v2 manifest (`aitools.manifest.json`, `SPEC-008-R4`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestV2 {
    pub id: String,
    pub name: String,
    pub version: String,
    pub runtime: PluginRuntimeTier,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub main: Option<String>,
    #[serde(default)]
    pub wasm_entry: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub ai_tools: Vec<AiToolDeclaration>,
    #[serde(default = "default_skill_file")]
    pub skill_file: String,
    #[serde(default)]
    pub features: Vec<Feature>,
    #[serde(default)]
    pub ai_quotas: Option<PluginAiQuotas>,
}

fn default_skill_file() -> String {
    "SKILL.md".to_string()
}

impl ManifestV2 {
    /// Validates required fields and runtime invariants (`SPEC-003-R2`, `SPEC-008-R4`).
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Manifest`] if required fields (`id`, `name`, `version`) are empty,
    /// `legacy-compat` is declared on a v2 manifest, or runtime-specific entry fields are missing.
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(AitoolsError::Manifest("id must not be empty".into()));
        }
        if self.name.trim().is_empty() {
            return Err(AitoolsError::Manifest("name must not be empty".into()));
        }
        if self.version.trim().is_empty() {
            return Err(AitoolsError::Manifest("version must not be empty".into()));
        }
        match self.runtime {
            PluginRuntimeTier::LegacyCompat => {
                return Err(AitoolsError::Manifest(
                    "v2 manifest must not declare legacy-compat runtime".into(),
                ));
            }
            PluginRuntimeTier::NativeUi => {
                if self.main.as_deref().is_none_or(|m| m.trim().is_empty()) {
                    return Err(AitoolsError::Manifest(
                        "native-ui runtime requires a non-empty main entry".into(),
                    ));
                }
            }
            PluginRuntimeTier::RustWasm => {
                if self
                    .wasm_entry
                    .as_deref()
                    .is_none_or(|w| w.trim().is_empty())
                {
                    return Err(AitoolsError::Manifest(
                        "rust-wasm runtime requires a non-empty wasm_entry".into(),
                    ));
                }
            }
            PluginRuntimeTier::Hybrid => {
                if self.main.as_deref().is_none_or(|m| m.trim().is_empty())
                    || self
                        .wasm_entry
                        .as_deref()
                        .is_none_or(|w| w.trim().is_empty())
                {
                    return Err(AitoolsError::Manifest(
                        "hybrid runtime requires both main and wasm_entry".into(),
                    ));
                }
            }
            PluginRuntimeTier::RustNative => {}
        }
        Ok(())
    }
}

/// Tool schema declared in `aitools.manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiToolDeclaration {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub risk_tier: RiskTier,
    #[serde(default)]
    pub input_schema: serde_json::Value,
}

/// Optional per-plugin AI rate & token quotas (`SPEC-007-R4`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAiQuotas {
    pub requests_per_minute: u32,
    pub tokens_per_day: u64,
}

/// Legacy v1 plugin manifest (parsed from `plugin.json`, `SPEC-003-R1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platform: Vec<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub main: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub preload: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default, rename = "pluginSetting")]
    pub plugin_setting: Option<PluginSetting>,
    #[serde(default)]
    pub features: Vec<Feature>,
    #[serde(default)]
    pub development: Option<Development>,
    #[serde(default)]
    pub tools: HashMap<String, ToolManifest>,
    #[serde(default)]
    pub providers: HashMap<String, ProviderManifest>,
    #[serde(default)]
    pub permissions: Vec<String>,
}

impl PluginManifest {
    /// Whether this legacy manifest requires a QuickJS preload worker (`SPEC-008-R2`).
    pub fn requires_quickjs_preload(&self) -> bool {
        self.preload
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty())
    }
}

/// Classification result for a plugin directory on disk (`SPEC-003-R3`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginClassification {
    pub id: String,
    pub tier: PluginRuntimeTier,
    pub requires_quickjs: bool,
    pub manifest_file: String,
}

/// Inspects a plugin directory and classifies its runtime tier (`SPEC-003-R3`).
///
/// # Errors
///
/// Returns [`AitoolsError::Io`] if reading a manifest fails, [`AitoolsError::Json`] if parsing
/// fails, or [`AitoolsError::Manifest`] if no manifest is found or validation fails.
pub fn classify_plugin_dir(plugin_dir: &Path) -> Result<PluginClassification> {
    let v2_path = plugin_dir.join("aitools.manifest.json");
    if v2_path.is_file() {
        let raw = std::fs::read_to_string(&v2_path)?;
        let manifest: ManifestV2 = serde_json::from_str(&raw)?;
        manifest.validate()?;
        return Ok(PluginClassification {
            id: manifest.id,
            tier: manifest.runtime,
            requires_quickjs: false,
            manifest_file: "aitools.manifest.json".to_string(),
        });
    }

    let v1_path = plugin_dir.join("plugin.json");
    if v1_path.is_file() {
        let raw = std::fs::read_to_string(&v1_path)?;
        let manifest: PluginManifest = serde_json::from_str(&raw)?;
        return Ok(PluginClassification {
            id: sanitize_plugin_id(&manifest.name),
            tier: PluginRuntimeTier::LegacyCompat,
            requires_quickjs: manifest.requires_quickjs_preload(),
            manifest_file: "plugin.json".to_string(),
        });
    }

    Err(AitoolsError::Manifest(format!(
        "no aitools.manifest.json or plugin.json found in {}",
        plugin_dir.display()
    )))
}

/// Normalizes a plugin name into a stable lowercase identifier.
pub fn sanitize_plugin_id(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "unnamed-plugin".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Provider declaration exposed by a v1 plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderManifest {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Tool declaration exposed inside a v1 `plugin.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifest {
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: serde_json::Value,
    #[serde(default, rename = "outputSchema")]
    pub output_schema: Option<serde_json::Value>,
}

/// Plugin-level window/singleton settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSetting {
    #[serde(default, deserialize_with = "deserialize_lenient_bool")]
    pub single: bool,
    #[serde(default = "default_height")]
    pub height: u32,
}

fn default_height() -> u32 {
    544
}

/// Development-time overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Development {
    #[serde(default)]
    pub main: Option<String>,
    #[serde(default)]
    pub preload: Option<String>,
}

/// Feature declaration inside a plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feature {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platform: Vec<String>,
    pub code: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub explain: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(
        default,
        rename = "mainPush",
        deserialize_with = "deserialize_lenient_bool"
    )]
    pub main_push: bool,
    #[serde(
        default,
        rename = "mainHide",
        deserialize_with = "deserialize_lenient_bool"
    )]
    pub main_hide: bool,
    #[serde(default)]
    pub cmds: Vec<Cmd>,
}

fn deserialize_lenient_bool<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match val {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(b)) => b,
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        Some(serde_json::Value::String(s)) => {
            let trimmed = s.trim().to_ascii_lowercase();
            !trimmed.is_empty() && trimmed != "false" && trimmed != "0" && trimmed != "no"
        }
        Some(_) => true,
    })
}

/// Command match rule (plain keyword text or typed matcher).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Cmd {
    Text(String),
    Typed(CmdTyped),
}

#[derive(Deserialize)]
struct CmdTypedWire {
    #[serde(rename = "type")]
    type_: CmdType,
    label: Option<String>,
    #[serde(
        default,
        rename = "match",
        deserialize_with = "deserialize_match_field"
    )]
    match_: Option<String>,
    #[serde(default, deserialize_with = "deserialize_match_field")]
    regex: Option<String>,
    #[serde(default)]
    exclude: Option<String>,
    #[serde(
        default,
        alias = "minLength",
        deserialize_with = "deserialize_length_field"
    )]
    length: Option<u32>,
    #[serde(
        default,
        rename = "maxLength",
        deserialize_with = "deserialize_length_field"
    )]
    max_length: Option<u32>,
    #[serde(default, rename = "fileType")]
    file_type: Option<String>,
    #[serde(default)]
    extensions: Option<Vec<String>>,
}

impl From<CmdTypedWire> for CmdTyped {
    fn from(wire: CmdTypedWire) -> Self {
        Self {
            type_: wire.type_,
            label: wire.label,
            match_: wire.match_.or(wire.regex),
            exclude: wire.exclude,
            length: wire.length,
            max_length: wire.max_length,
            file_type: wire.file_type,
            extensions: wire.extensions,
        }
    }
}

/// Typed command rule (`SPEC-003-R1`, `SPEC-005`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "CmdTypedWire")]
pub struct CmdTyped {
    #[serde(rename = "type")]
    pub type_: CmdType,
    pub label: Option<String>,
    #[serde(
        default,
        rename = "match",
        alias = "regex",
        deserialize_with = "deserialize_match_field"
    )]
    pub match_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<String>,
    #[serde(
        default,
        alias = "minLength",
        deserialize_with = "deserialize_length_field"
    )]
    pub length: Option<u32>,
    #[serde(
        default,
        rename = "maxLength",
        deserialize_with = "deserialize_length_field"
    )]
    pub max_length: Option<u32>,
    #[serde(default, rename = "fileType", skip_serializing_if = "Option::is_none")]
    pub file_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
}

fn deserialize_match_field<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match val {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s),
        Some(other) => Some(other.to_string()),
    })
}

fn deserialize_length_field<'de, D>(deserializer: D) -> std::result::Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match val {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
            .or_else(|| n.as_i64().map(|v| v.max(0) as u32)),
        Some(serde_json::Value::String(s)) => s.trim().parse::<u32>().ok(),
        _ => None,
    })
}

/// Command matcher category.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CmdType {
    Regex,
    Over,
    Img,
    Files,
    Window,
}

/// Installed plugin record stored in LMDB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRecord {
    pub id: String,
    pub name: String,
    pub version: String,
    pub path: String,
    pub enabled: bool,
    pub tier: PluginRuntimeTier,
    pub manifest_json: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

/// Feature entry joined with owning plugin metadata for search & palette display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureEntry {
    pub code: String,
    pub plugin_id: String,
    pub plugin_name: String,
    pub label: String,
    pub icon: Option<String>,
    pub explain: String,
    pub cmds: Vec<Cmd>,
    pub main_push: bool,
    pub priority: i32,
}

/// PouchDB-compatible plugin document (`SPEC-002-R1`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Document {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_rev", default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(flatten)]
    pub data: serde_json::Value,
}

/// Locally stored clipboard history entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardHistoryEntry {
    pub id: String,
    pub text: String,
    pub first_copied_at: String,
    pub last_copied_at: String,
    pub used_count: u32,
}
