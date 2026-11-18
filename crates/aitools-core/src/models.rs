//! Unified plugin manifests (ZTools/uTools v1 & `aitools` v2), features, and documents (`SPEC-003`, `SPEC-008`, `SPEC-015`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

/// Legacy v1 plugin manifest (parsed from `plugin.json`, `SPEC-003-R1`, `SPEC-015-R1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "PluginManifestWire")]
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

#[derive(Deserialize)]
struct PluginManifestWire {
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    platform: Vec<String>,
    name: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "pluginName")]
    plugin_name: Option<String>,
    #[serde(default)]
    version: String,
    #[serde(default)]
    main: Option<String>,
    #[serde(default)]
    logo: Option<String>,
    #[serde(default)]
    preload: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default, rename = "pluginSetting")]
    plugin_setting: Option<PluginSetting>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    features: Vec<Feature>,
    #[serde(default)]
    development: Option<Development>,
    #[serde(default)]
    tools: HashMap<String, ToolManifest>,
    #[serde(default)]
    providers: HashMap<String, ProviderManifest>,
    #[serde(default)]
    permissions: Vec<String>,
}

impl From<PluginManifestWire> for PluginManifest {
    fn from(wire: PluginManifestWire) -> Self {
        let title = wire
            .title
            .filter(|s| !s.trim().is_empty())
            .or_else(|| wire.plugin_name.filter(|s| !s.trim().is_empty()));
        let plugin_setting = match (wire.plugin_setting, wire.height) {
            (Some(setting), _) => Some(setting),
            (None, Some(h)) => Some(PluginSetting {
                single: false,
                height: h,
            }),
            (None, None) => None,
        };
        Self {
            platform: wire.platform,
            name: wire.name,
            title,
            version: wire.version,
            main: wire.main,
            logo: wire.logo,
            preload: wire.preload,
            description: wire.description,
            author: wire.author,
            homepage: wire.homepage,
            plugin_setting,
            features: wire.features,
            development: wire.development,
            tools: wire.tools,
            providers: wire.providers,
            permissions: wire.permissions,
        }
    }
}

impl PluginManifest {
    /// Whether this legacy manifest requires a QuickJS preload worker (`SPEC-008-R2`).
    pub fn requires_quickjs_preload(&self) -> bool {
        self.effective_preload().is_some()
    }

    /// Returns the human-readable display title (`title` / `pluginName` / `name`).
    #[must_use]
    pub fn display_title(&self) -> &str {
        self.title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(self.name.as_str())
    }

    /// Returns the active HTML/URL entrypoint, respecting read-only `development.main`.
    #[must_use]
    pub fn effective_main(&self) -> Option<&str> {
        self.main
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.development
                    .as_ref()
                    .and_then(|d| d.main.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
    }

    /// Returns the active preload script relative path, respecting read-only `development.preload`.
    #[must_use]
    pub fn effective_preload(&self) -> Option<&str> {
        self.preload
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.development
                    .as_ref()
                    .and_then(|d| d.preload.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
    }

    /// Returns the configured initial window height in logical pixels (`544` default).
    #[must_use]
    pub fn effective_height(&self) -> u32 {
        self.plugin_setting
            .as_ref()
            .map_or_else(default_height, |s| s.height)
    }

    /// Whether this plugin requests a singleton window (`pluginSetting.single`).
    #[must_use]
    pub fn effective_single(&self) -> bool {
        self.plugin_setting.as_ref().is_some_and(|s| s.single)
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

/// Inspects a plugin directory and classifies its runtime tier (`SPEC-003-R3`, `SPEC-015-R1`).
///
/// # Errors
///
/// Returns [`AitoolsError::Io`] if reading a manifest fails, [`AitoolsError::Json`] if parsing
/// fails, or [`AitoolsError::Manifest`] if no manifest is found or validation fails.
pub fn classify_plugin_dir(plugin_dir: &Path) -> Result<PluginClassification> {
    let loaded = load_plugin_dir(plugin_dir)?;
    let requires_quickjs = loaded.tier == PluginRuntimeTier::LegacyCompat
        && loaded
            .preload
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty());
    Ok(PluginClassification {
        id: loaded.id,
        tier: loaded.tier,
        requires_quickjs,
        manifest_file: loaded.manifest_file,
    })
}

/// Normalizes a plugin name into a stable lowercase identifier.
///
/// Purely non-ASCII names derive a collision-free deterministic `plugin-<short-sha256>`
/// identifier (`SPEC-015-R1`) instead of collapsing to `"unnamed-plugin"`.
#[must_use]
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
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    let raw_trimmed = name.trim();
    if raw_trimmed.is_empty() {
        return "unnamed-plugin".to_string();
    }
    let digest = Sha256::digest(raw_trimmed.as_bytes());
    format!(
        "plugin-{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    )
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
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_string_or_vec"
    )]
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

fn deserialize_string_or_vec<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match val {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                vec![trimmed.to_string()]
            }
        }
        Some(serde_json::Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| match item {
                serde_json::Value::String(s) => {
                    let t = s.trim();
                    (!t.is_empty()).then(|| t.to_string())
                }
                _ => None,
            })
            .collect(),
        Some(_) => Vec::new(),
    })
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
        rename = "minNum",
        deserialize_with = "deserialize_length_field"
    )]
    min_num: Option<u32>,
    #[serde(
        default,
        rename = "maxLength",
        deserialize_with = "deserialize_length_field"
    )]
    max_length: Option<u32>,
    #[serde(
        default,
        rename = "maxNum",
        deserialize_with = "deserialize_length_field"
    )]
    max_num: Option<u32>,
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
            length: wire.length.or(wire.min_num),
            max_length: wire.max_length.or(wire.max_num),
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

/// PouchDB-compatible plugin document (`SPEC-002-R1`, `SPEC-016-R5`).
///
/// Accepts, persists, and serializes both flat ZTools/PouchDB fields (`{ _id, text }`) and
/// `@aitools/sdk` envelope payloads (`{ _id, data: { text } }`) without field loss.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Document {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_rev", default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(flatten)]
    pub data: serde_json::Value,
}

impl Document {
    /// Returns the inner `data` object when stored as an `@aitools/sdk` envelope (`{ _id, data: {...} }`),
    /// or the flattened field object itself for flat ZTools/PouchDB documents.
    #[must_use]
    pub fn payload(&self) -> &serde_json::Value {
        if let Some(obj) = self.data.as_object() {
            if obj.len() == 1 {
                if let Some(inner) = obj.get("data") {
                    if inner.is_object() {
                        return inner;
                    }
                }
            }
        }
        &self.data
    }

    /// Looks up a field across top-level flat document fields and nested `{ data }` envelopes.
    #[must_use]
    pub fn get_field(&self, key: &str) -> Option<&serde_json::Value> {
        self.data
            .get(key)
            .or_else(|| self.data.get("data").and_then(|inner| inner.get(key)))
    }
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

const MAX_PLUGIN_ICON_BYTES: usize = 512 * 1024;

/// Unified read-only representation of a loaded v1 (`plugin.json`) or v2 (`aitools.manifest.json`) plugin directory (`SPEC-015-R1`, `SPEC-016-R1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedPlugin {
    /// Normalized unique plugin identifier (`id` for v2, `sanitize_plugin_id(name)` for v1).
    pub id: String,
    /// Human-readable display title.
    pub name: String,
    /// Semantic or manifest version string.
    pub version: String,
    /// Optional plugin description.
    pub description: Option<String>,
    /// Classified execution tier.
    pub tier: PluginRuntimeTier,
    /// Canonical filesystem directory containing the plugin.
    pub root_dir: PathBuf,
    /// Manifest filename (`"aitools.manifest.json"` or `"plugin.json"`).
    pub manifest_file: String,
    /// Raw parsed manifest JSON preserved read-only.
    pub manifest_json: serde_json::Value,
    /// Entry HTML file or external `http(s)://` URL.
    pub main: Option<String>,
    /// Preload script path relative to `root_dir`, if declared.
    pub preload: Option<String>,
    /// Wasm entry path relative to `root_dir`, if declared.
    pub wasm_entry: Option<String>,
    /// Resolved icon (`data:` URI, `http(s)://` URL, or base64 data URI from a local icon file).
    pub icon: Option<String>,
    /// Whether the plugin requests singleton window mode.
    pub single_window: bool,
    /// Initial window width in logical pixels.
    pub window_width: u32,
    /// Initial window height in logical pixels.
    pub window_height: u32,
    /// Declared capability permissions (`SPEC-016-R4`).
    pub permissions: Vec<String>,
    /// Parsed v1 manifest when loaded from `plugin.json`.
    pub v1: Option<PluginManifest>,
    /// Parsed v2 manifest when loaded from `aitools.manifest.json`.
    pub v2: Option<ManifestV2>,
}

/// Returns the current host OS tag matching ZTools `platform` strings (`"darwin"`, `"win32"`, `"linux"`).
#[must_use]
pub const fn current_ztools_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "darwin"
    }
    #[cfg(target_os = "windows")]
    {
        "win32"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "linux"
    }
}

/// Checks whether a ZTools `platform` filter list allows `current_platform`.
///
/// An empty slice allows all platforms.
#[must_use]
pub fn platform_matches(platforms: &[String], current_platform: &str) -> bool {
    if platforms.is_empty() {
        return true;
    }
    let cur = current_platform.to_ascii_lowercase();
    platforms.iter().any(|p| {
        let norm = p.trim().to_ascii_lowercase();
        match cur.as_str() {
            "darwin" => matches!(norm.as_str(), "darwin" | "macos" | "mac"),
            "win32" => matches!(norm.as_str(), "win32" | "windows" | "win"),
            "linux" => norm == "linux",
            other => norm == other,
        }
    })
}

/// Decodes raw manifest file bytes supporting UTF-8, UTF-8 BOM, UTF-16LE BOM, and UTF-16BE BOM (`SPEC-015-R1`).
///
/// # Errors
///
/// Returns [`AitoolsError::Manifest`] if the byte sequence has odd UTF-16 byte length or invalid
/// UTF-8 / UTF-16 encoding.
pub fn decode_manifest_bytes(bytes: &[u8]) -> Result<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(rest.to_vec())
            .map_err(|e| AitoolsError::Manifest(format!("invalid UTF-8 manifest: {e}")));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        if rest.len() % 2 != 0 {
            return Err(AitoolsError::Manifest(
                "invalid UTF-16LE manifest length".into(),
            ));
        }
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&units)
            .map_err(|e| AitoolsError::Manifest(format!("invalid UTF-16LE manifest: {e}")));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        if rest.len() % 2 != 0 {
            return Err(AitoolsError::Manifest(
                "invalid UTF-16BE manifest length".into(),
            ));
        }
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&units)
            .map_err(|e| AitoolsError::Manifest(format!("invalid UTF-16BE manifest: {e}")));
    }
    String::from_utf8(bytes.to_vec())
        .map_err(|e| AitoolsError::Manifest(format!("invalid UTF-8 manifest: {e}")))
}

/// Resolves a manifest `logo` or feature `icon` read-only into a renderable URI (`data:` or `http(s)://`).
///
/// Relative asset paths are verified to stay within `plugin_dir` and converted to `data:<mime>;base64,...`
/// without altering any file on disk (`SPEC-015-R1`). Returns `None` if empty, missing, or oversized.
#[must_use]
pub fn resolve_plugin_icon(plugin_dir: &Path, raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("data:")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        return Some(trimmed.to_string());
    }

    let rel = trimmed.trim_start_matches("./").trim_start_matches('/');
    let root_canon = std::fs::canonicalize(plugin_dir).ok()?;
    let target_canon = std::fs::canonicalize(root_canon.join(rel)).ok()?;
    if !target_canon.starts_with(&root_canon) || !target_canon.is_file() {
        return None;
    }
    let meta = std::fs::metadata(&target_canon).ok()?;
    if meta.len() == 0 || meta.len() as usize > MAX_PLUGIN_ICON_BYTES {
        return None;
    }
    let bytes = std::fs::read(&target_canon).ok()?;
    let mime = match target_canon
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        _ => return None,
    };
    Some(format!(
        "data:{mime};base64,{}",
        BASE64_STANDARD.encode(&bytes)
    ))
}

/// Loads a v2 (`aitools.manifest.json`) or legacy v1 (`plugin.json`) plugin directory read-only (`SPEC-015-R1`).
///
/// # Errors
///
/// Returns [`AitoolsError::Io`] if reading files fails, [`AitoolsError::Json`] if JSON parsing
/// fails, or [`AitoolsError::Manifest`] if no manifest exists, decoding fails, or validation fails.
pub fn load_plugin_dir(plugin_dir: &Path) -> Result<LoadedPlugin> {
    let root_dir = std::fs::canonicalize(plugin_dir).unwrap_or_else(|_| plugin_dir.to_path_buf());
    let v2_path = root_dir.join("aitools.manifest.json");
    if v2_path.is_file() {
        let raw = decode_manifest_bytes(&std::fs::read(&v2_path)?)?;
        let manifest_json: serde_json::Value = serde_json::from_str(&raw)?;
        let manifest: ManifestV2 = serde_json::from_str(&raw)?;
        manifest.validate()?;
        let icon = resolve_plugin_icon(
            &root_dir,
            manifest_json
                .get("icon")
                .or_else(|| manifest_json.get("logo"))
                .and_then(serde_json::Value::as_str),
        );
        return Ok(LoadedPlugin {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            tier: manifest.runtime,
            root_dir,
            manifest_file: "aitools.manifest.json".to_string(),
            manifest_json,
            main: manifest
                .main
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            preload: None,
            wasm_entry: manifest
                .wasm_entry
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            icon,
            single_window: false,
            window_width: 800,
            window_height: default_height(),
            permissions: manifest.permissions.clone(),
            v1: None,
            v2: Some(manifest),
        });
    }

    let v1_path = root_dir.join("plugin.json");
    if v1_path.is_file() {
        let raw = decode_manifest_bytes(&std::fs::read(&v1_path)?)?;
        let manifest_json: serde_json::Value = serde_json::from_str(&raw)?;
        let manifest: PluginManifest = serde_json::from_str(&raw)?;
        let id = sanitize_plugin_id(&manifest.name);
        let name = manifest.display_title().to_string();
        let version = if manifest.version.trim().is_empty() {
            "0.0.0".to_string()
        } else {
            manifest.version.clone()
        };
        let main = manifest.effective_main().map(str::to_string);
        let preload = manifest.effective_preload().map(str::to_string);
        let icon = resolve_plugin_icon(&root_dir, manifest.logo.as_deref());
        let window_width = manifest_json
            .get("width")
            .and_then(serde_json::Value::as_u64)
            .and_then(|w| u32::try_from(w).ok())
            .unwrap_or(800);
        let window_height = manifest.effective_height();
        let single_window = manifest.effective_single();
        let permissions = manifest.permissions.clone();
        let description = manifest.description.clone();
        return Ok(LoadedPlugin {
            id,
            name,
            version,
            description,
            tier: PluginRuntimeTier::LegacyCompat,
            root_dir,
            manifest_file: "plugin.json".to_string(),
            manifest_json,
            main,
            preload,
            wasm_entry: None,
            icon,
            single_window,
            window_width,
            window_height,
            permissions,
            v1: Some(manifest),
            v2: None,
        });
    }

    Err(AitoolsError::Manifest(format!(
        "no aitools.manifest.json or plugin.json found in {}",
        plugin_dir.display()
    )))
}

/// Scans immediate subdirectories of `root` for valid v1 or v2 plugins read-only (`SPEC-015-R1`, `SPEC-016-R1`).
///
/// Missing `root` returns `Ok(Vec::new())`. Subdirectories without manifests or with malformed
/// manifests are skipped cleanly with a `tracing::warn!` record rather than aborting the scan.
///
/// # Errors
///
/// Returns [`AitoolsError::Io`] if reading the top-level `root` directory entries fails.
pub fn scan_plugins_dir(root: &Path) -> Result<Vec<LoadedPlugin>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(root)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| !name.starts_with('.') && name != "node_modules")
        })
        .collect();
    subdirs.sort();

    let mut plugins = Vec::new();
    for dir in subdirs {
        if !dir.join("aitools.manifest.json").is_file() && !dir.join("plugin.json").is_file() {
            continue;
        }
        match load_plugin_dir(&dir) {
            Ok(loaded) => plugins.push(loaded),
            Err(err) => {
                tracing::warn!(dir = %dir.display(), error = %err, "skipping unreadable plugin directory");
            }
        }
    }
    Ok(plugins)
}

impl LoadedPlugin {
    /// Converts this loaded plugin into searchable [`FeatureEntry`] items for the current OS platform.
    #[must_use]
    pub fn to_feature_entries(&self) -> Vec<FeatureEntry> {
        self.to_feature_entries_for_platform(Some(current_ztools_platform()))
    }

    /// Converts this loaded plugin into searchable [`FeatureEntry`] items, optionally filtering by `platform`.
    #[must_use]
    pub fn to_feature_entries_for_platform(&self, platform: Option<&str>) -> Vec<FeatureEntry> {
        if let Some(ref v1) = self.v1 {
            if let Some(cur) = platform {
                if !platform_matches(&v1.platform, cur) {
                    return Vec::new();
                }
            }
            return self.convert_features(&v1.features, platform, &[]);
        }
        if let Some(ref v2) = self.v2 {
            let tool_names: Vec<String> = v2.ai_tools.iter().map(|t| t.name.clone()).collect();
            return self.convert_features(&v2.features, platform, &tool_names);
        }
        Vec::new()
    }

    fn convert_features(
        &self,
        features: &[Feature],
        platform: Option<&str>,
        extra_keywords: &[String],
    ) -> Vec<FeatureEntry> {
        let mut out = Vec::new();
        for feature in features {
            if let Some(cur) = platform {
                if !platform_matches(&feature.platform, cur) {
                    continue;
                }
            }
            let label = feature
                .label
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    feature.cmds.iter().find_map(|c| match c {
                        Cmd::Text(t) if !t.trim().is_empty() => Some(t.trim().to_string()),
                        Cmd::Typed(t) => t
                            .label
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                        Cmd::Text(_) => None,
                    })
                })
                .or_else(|| {
                    let trimmed = feature.explain.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                })
                .unwrap_or_else(|| self.name.clone());

            let icon = resolve_plugin_icon(&self.root_dir, feature.icon.as_deref())
                .or_else(|| self.icon.clone());
            let explain = if feature.explain.trim().is_empty() {
                self.description.clone().unwrap_or_else(|| label.clone())
            } else {
                feature.explain.clone()
            };
            let cmds = if feature.cmds.is_empty() {
                vec![Cmd::Text(label.clone())]
            } else {
                feature.cmds.clone()
            };

            out.push(FeatureEntry {
                code: feature.code.clone(),
                plugin_id: self.id.clone(),
                plugin_name: self.name.clone(),
                label,
                icon,
                explain,
                cmds,
                main_push: feature.main_push,
                priority: 10,
            });
        }

        if out.is_empty() && features.is_empty() {
            let mut seen = HashSet::new();
            let mut cmds = Vec::new();
            let mut push_kw = |kw: &str| {
                let trimmed = kw.trim();
                if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
                    cmds.push(Cmd::Text(trimmed.to_string()));
                }
            };
            push_kw(&self.id);
            push_kw(&self.name);
            for part in self.id.split(['-', '_']) {
                if part.len() >= 2 {
                    push_kw(part);
                }
            }
            for word in self.name.split_whitespace() {
                if word.len() >= 2 {
                    push_kw(word);
                }
            }
            for tool in extra_keywords {
                push_kw(tool);
            }
            out.push(FeatureEntry {
                code: self.id.clone(),
                plugin_id: self.id.clone(),
                plugin_name: self.name.clone(),
                label: self.name.clone(),
                icon: self.icon.clone(),
                explain: self
                    .description
                    .clone()
                    .unwrap_or_else(|| self.name.clone()),
                cmds,
                main_push: false,
                priority: 20,
            });
        }

        out
    }

    /// Converts this loaded plugin into a [`PluginRecord`] suitable for LMDB persistence.
    #[must_use]
    pub fn to_plugin_record(&self, timestamp: &str) -> PluginRecord {
        PluginRecord {
            id: self.id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            path: self.root_dir.to_string_lossy().into_owned(),
            enabled: true,
            tier: self.tier,
            manifest_json: self.manifest_json.clone(),
            created_at: timestamp.to_string(),
            updated_at: timestamp.to_string(),
        }
    }
}
