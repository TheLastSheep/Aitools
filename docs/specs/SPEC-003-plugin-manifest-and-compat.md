# SPEC-003: Unified Plugin Manifests & ZTools/uTools Compatibility

- **Status:** Active
- **Version:** 1.0.0
- **Owner Crate:** `crates/aitools-core`

## 1. Legacy v1 (`plugin.json`) & Native v2 (`aitools.manifest.json`) Parsing

- **SPEC-003-R1 (Real-World ZTools/uTools Manifest Quirks):** Parsing `plugin.json` MUST faithfully handle legacy ecosystem quirks without failing:
  - Lenient booleans on `mainPush`, `mainHide`, and `pluginSetting.single` (accepting booleans, integers, and strings including community typos such as `"ture"` while rejecting `"false"`, `"0"`, `"no"`).
  - Polymorphic `match` fields on `window` and `files` rules (accepting either string literals or JSON objects serialized compactly).
  - Length field aliases (`minLength` alias for `length`, and stringified integers for `length` / `maxLength`).
  - `regex` alias fallback when `match` is omitted on typed `regex` commands (`match` takes precedence if present).
- **SPEC-003-R2 (Manifest v2 Schema & Validation):** `aitools.manifest.json` MUST validate `id`, `name`, `version`, `runtime` (`native-ui` | `rust-wasm` | `hybrid`), `permissions`, `ai_tools`, and optional `skill_file` (default `"SKILL.md"`), matching `SPEC-008-R4`.
- **SPEC-003-R3 (Deterministic Runtime Tier Classification):** Given a plugin directory, `classify_plugin_dir` MUST deterministically classify it as `LegacyCompat` (`plugin.json`), `NativeUi` (`aitools.manifest.json` with `native-ui`), `RustWasm` (`aitools.manifest.json` with `rust-wasm`), or `Hybrid`, and inspect whether `"preload"` is present (`SPEC-008-R2`).

## 2. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Implemented in `crates/aitools-core/src/models.rs` and verified:
  - `SPEC-003-R1`: Salvaged v1 `PluginManifest`, `CmdTyped` (`CmdTypedWire`), and lenient deserializers — verified by `spec_003_r1_ztools_manifest_quirks_parse_cleanly` (`crates/aitools-core/tests/parity.rs`).
  - `SPEC-003-R2` & `SPEC-003-R3`: `ManifestV2::validate` and `classify_plugin_dir` exercised by `crates/aitools-cli` (`spec_009_r1_init_audit_and_deterministic_pack`) and `crates/aitools-js-bridge` (`spec_008_r2_skips_quickjs_without_preload`).
