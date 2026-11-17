# SPEC-002: LMDB Storage Engine & Synchronous Plugin Sync Protocol

- **Status:** Active
- **Version:** 1.0.0
- **Owner Crate:** `crates/aitools-core`

## 1. Embedded LMDB Engine (`heed`)

- **SPEC-002-R1 (PouchDB-Compatible Document Store):** `StorageEngine` MUST persist per-plugin documents (`_id`, `_rev`, JSON object fields) in LMDB (`heed`) with atomic revision checks (`1-<hash>` style monotonic/content-addressed revs) and conflict detection (`DocumentConflict`) without blocking concurrent reads.
- **SPEC-002-R2 (Bounded Transport & Storage Quotas):** Every plugin document operation MUST enforce:
  - `MAX_DOCUMENT_BYTES = 262_144` (256 KiB)
  - `MAX_RESPONSE_BYTES = 524_288` (512 KiB)
  - `MAX_QUERY_DOCUMENTS = 1000`
  - `MAX_KEYS_SELECTOR = 128`
  Duplicate IDs in `bulkDocs` or budget overflows MUST reject before commit.
- **SPEC-002-R3 (Attachment Isolation & Cascade):** Binary attachments MUST store bounded metadata and payload separately per `(plugin_id, attachment_id)`, and uninstalling a plugin MUST cascade-delete its documents, attachments, and settings.

## 2. Protocol Caller Verification (`INV-SEC-01`)

- **SPEC-002-R4 (Sync Caller Isolation):** Every `aitools-sync://` or `aitools-plugin://` request MUST validate `webview_label` (`plugin-<id>`) against the target `plugin_id` and reject cross-plugin or non-plugin callers with HTTP `403` (`Forbidden`).

## 3. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Implemented on `heed` LMDB in `crates/aitools-core/src/storage.rs` and verified:
  - `SPEC-002-R1` & `SPEC-002-R3`: `StorageEngine` (`put_doc`, `get_doc`, `remove_doc`, `put_attachment`, `get_attachment`, `uninstall_plugin_cascade`) — verified by `spec_002_r1_atomic_revision_conflict_and_cascade`.
  - `SPEC-002-R2`: `bulk_docs` duplicate-ID preflight & byte/count bounds — verified by `spec_002_r2_enforces_budgets_and_rejects_bulk_duplicates`.
  - `SPEC-002-R4`: `validate_sync_caller` — verified by `spec_002_r4_validates_sync_caller_webview_label`.
- **Measured Latency (`wake_latency` bench):** `StorageEngine::get_doc` measured `P50 = 0 us`, `P99 = 3 us`.
