# Salvage & Clean Workspace Report (`atools` -> `aitools`)

## 1. Architectural Transition
`/Users/harris/Desktop/atools` accumulated ~943 Rust source files, a 420 KB monolithic `commands.rs`, SQLite-per-IPC bloat, `#![allow(clippy::...)]` suppressions, and sprawling multi-renderer experiments. `/Users/harris/Desktop/aitools` is rebuilt strictly under Spec-Driven Development (`AGENTS.md`, `SPEC-000`–`SPEC-010`) with a strict acyclic Crate DAG, LMDB (`heed`), `nucleo-matcher`, `rquickjs` (0.8), `wasmtime` (27), and chronological `aitools-telemetry` observability.

## 2. Salvaged Components & Parity Assets

| Source in `/Users/harris/Desktop/atools` | Destination in `/Users/harris/Desktop/aitools` | Governing Spec | Notes |
| :--- | :--- | :--- | :--- |
| `crates/atools-core/src/models.rs` + `models/typed_command.rs` | `crates/aitools-core/src/models.rs` | `SPEC-003` | Preserved ZTools quirks (`deserialize_lenient_bool`, `deserialize_match_field`, `deserialize_length_field`, `minLength`/`regex` aliases); added `ManifestV2` & `PluginRuntimeTier`. |
| `crates/atools-core/src/matcher/` (`index.rs`, `typed_text_*`, `media_*`) | `crates/aitools-core/src/matcher/` | `SPEC-005` | Preserved 20,902+ CJK Pinyin initials/full pinyin, JS `/pattern/flags` parser, UTF-16 length metrics, bounded DFA regex banks, and added `nucleo-matcher` hybrid fuzzy fallback. |
| `crates/atools-core/src/db/plugin_data_contract.rs` | `crates/aitools-core/src/storage.rs` | `SPEC-002` | Preserved PouchDB `_id`/`_rev` conflict semantics and 256 KiB / 512 KiB / 1000-doc / 128-key budgets; re-implemented natively on `heed` LMDB + `INV-SEC-01` caller verification. |
| `tests/fixtures/launcher-typed-text-contract-cases.json`, `launcher-media-contract-cases.json` | `crates/aitools-core/tests/fixtures/` + `tests/parity.rs` | `SPEC-003`, `SPEC-005` | Full contract parity suite executed via `cargo test -p aitools-core --test parity`. |
| `src-tauri/src/hotkey/double_tap.rs`, `parser.rs` | `crates/aitools-native/src/hotkey.rs` | `SPEC-001` | Preserved pure monotonic-clock double-modifier edge detector and hotkey label validator. |
| `src-tauri/src/launcher_app_catalog.rs` | `crates/aitools-native/src/app_catalog.rs` | `SPEC-001` | Preserved bounded cross-platform app scanner (`MAX_DEPTH=4`, `MAX_ENTRIES=4096`, `MAX_APPS=1024`, 30s cache TTL). |
| `crates/atools-core/src/harness.rs` | `crates/aitools-ai/src/harness.rs` | `SPEC-007` | Salvaged `ErrorObservation`, `DangerousCommandGuard`, `PromptSanitizer`, `prune_tool_output` (Head 60% + Tail 40%), and `canonicalize_json` (KV-cache prefix stability). |
| `crates/atools-plugin/src/bridge.rs` | `crates/aitools-js-bridge/src/shim.rs` + `lazy_worker.rs` | `SPEC-008` | Distilled into frozen triple-global shim (`window.aitools === window.ztools === window.utools`) + lazy `rquickjs` 0.8 preload worker (`SPEC-008-R2`). |

## 3. Intentionally Left Behind (Not Salvaged)
- Monolithic `src-tauri/src/commands.rs` (420 KB) and `desktop_smoke.rs` (203 KB).
- Unbounded `node_modules/`, `dist/`, `ZTools-plugins/` (1.2 GB), `packages/ztools-settings/` (192 MB).
- Experimental `tauri-nspanel` forks, ad-hoc WKNavigationDelegate patches, and `#![allow(clippy::...)]` lint suppressions.
- SQLite `rusqlite` plugin data tables (replaced by zero-copy `heed` LMDB per `SPEC-000` and `SPEC-002`).
