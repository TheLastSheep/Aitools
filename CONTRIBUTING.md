# Contributing to aitools

`aitools` strictly enforces **Spec-Driven Development (SDD)** (`AGENTS.md`). Every behavioral change begins and ends with traceable specifications, automated tests, and chronological telemetry verification.

---

## 1. Spec-Driven Development Workflow (`SPEC-000`–`SPEC-014`)

1. **Spec Before Code (`AGENTS.md` Law #1):** Inspect the relevant specification in `docs/specs/` (see Section 4 of `AGENTS.md` for the full `SPEC-000`–`SPEC-014` index) before implementing features or modifying architecture boundaries. Update or propose spec clauses first if introducing new contracts.
2. **Traceable Test Naming (`SPEC-000` Section 4):** Every numbered requirement (`SPEC-XXX-RY`) MUST have an automated test whose function name includes `spec_xxx_ry` (for example, `spec_012_r2_sdk_commands_enforce_plugin_isolation_and_persist_docs`).
3. **Spec After Acceptance (`AGENTS.md` Law #2):** After tests pass, update the **Implementation & Verification Status** section at the bottom of the corresponding `SPEC-*.md` file with exact verified symbol and test names.

---

## 2. Crate Dependency DAG & Code Quality Invariants

Dependencies MUST remain strictly acyclic in this topological order (`SPEC-000`):

1. `aitools-telemetry` (zero internal workspace deps; initialized first)
2. `aitools-core` -> `aitools-telemetry`
3. `aitools-native` -> `aitools-telemetry`
4. `aitools-ai`, `aitools-js-bridge`, `aitools-wasm-host` -> `aitools-core`, `aitools-telemetry`
5. `tauri-plugin-aitools` -> host integration layer
6. `src-tauri` & `aitools-cli` -> top-level binary entrypoints

### Mandatory Rust Rules
- **Zero `.unwrap()` / panic paths in library code:** Never call `.unwrap()` outside `#[cfg(test)]` modules or `main()`. Return typed `thiserror` errors instead.
- **Documentation comments:** Every public API, struct, and error-returning function MUST carry `///` rustdoc comments (with `# Errors` and `# Panics` sections where applicable). `RUSTDOCFLAGS="-D warnings"` must pass with zero warnings.
- **Security invariants (`INV-SEC-01`..`04`):** Never bypass webview caller checks, post-load `eval()` script injection rules, OS Keychain isolation, or `PolicyGuard` HITL gates.

---

## 3. Recording & Replaying `.ztrace` Regressions via DSH

Whenever debugging latency spikes, IPC races, or plugin errors:

1. Open the **Developer & Diagnostic Harness (DSH)** panel in the desktop shell using `Cmd+Shift+D` (`SPEC-009`, `SPEC-014`).
2. Reproduce the flow (optionally enabling deterministic faults via `dsh_set_fault_injection`).
3. Export a redacted diagnostic bundle (`aitools-diag-*.zip`) via DSH or CLI:
   ```bash
   cargo run -p aitools-cli -- logs export --log-dir <app_log_dir> --out diag.zip --zip --trace-id <trace_id>
   ```
4. Inspect slow spans and anomaly snapshots (`active_plugin_count`, `slowest_subspan`) with `aitools doctor`:
   ```bash
   cargo run -p aitools-cli -- doctor --log-dir <app_log_dir>
   ```
5. Convert the bundle into a deterministic replay fixture and verify zero anomalies:
   ```bash
   cargo run -p aitools-cli -- harness import-diag diag.zip --trace-id <trace_id> --out tests/traces/my_repro.ztrace
   cargo run -p aitools-cli -- harness replay tests/traces/my_repro.ztrace --assert-no-anomalies
   ```
6. Note that root `.gitignore` ignores ad-hoc `*.ztrace` files while allowing curated CI fixtures under `!tests/traces/*.ztrace`.

---

## 4. Mandatory PR Quality Gates (`.github/workflows/sdd-gate.yml`)

All commands below MUST pass locally before opening or merging any Pull Request:

```bash
# Rust format, lint, documentation, tests, and performance benchmark
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo test --workspace
cargo bench -p aitools-core --bench wake_latency

# Golden .ztrace deterministic replay suite
for f in tests/traces/*.ztrace; do
  cargo run -p aitools-cli -- harness replay "$f" --assert-no-anomalies
done

# Built-in & example plugin audits
for p in plugins/builtin/* examples/plugins/*; do
  cargo run -p aitools-cli -- plugin audit "$p"
done

# Svelte frontend & @aitools/sdk TypeScript checks
pnpm --filter @aitools/renderer-svelte check
pnpm --filter @aitools/renderer-svelte build
./packages/renderer-svelte/node_modules/.bin/tsc -p packages/aitools-sdk/tsconfig.json --noEmit
```

If modifying `Cargo.toml` release settings or desktop dependencies, also run `pnpm tauri build` and confirm the output installer remains `<= 18 MiB` (`SPEC-000`).

---

## 5. Commit Convention

Use atomic, single-concern commits following `<type>(<scope>): <subject>`:
- Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`
- Include clear module scopes (`core`, `ai`, `cli`, `desktop`, `sdk`, `ci`, `docs`).
