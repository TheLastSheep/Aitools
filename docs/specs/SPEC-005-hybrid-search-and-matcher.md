# SPEC-005: Hybrid Search, CJK Pinyin & Typed Command Matcher

- **Status:** Active
- **Version:** 1.0.0
- **Owner Crate:** `crates/aitools-core`

## 1. Zero-Allocation Deterministic Fast Path (`SPEC-007-R1`)

- **SPEC-005-R1 (Full Unicode CJK Pinyin + Zero-Alloc ASCII):** `FeatureSearchIndex` MUST precompute lowercase labels, 20,902+ CJK Pinyin initials (`to_pinyin_initials`), and full toneless Pinyin (`to_pinyin_full`) once at index build time so per-keystroke ASCII and Pinyin matching performs zero per-candidate heap allocations.
- **SPEC-005-R2 (Hybrid Fuzzy Scoring):** When exact, prefix, substring, or Pinyin matches do not exhaust candidates, `FeatureSearchIndex` MUST fall back to `nucleo-matcher` fuzzy ranking while ranking exact > prefix > regex > img > files > over > substring > pinyin > fuzzy.

## 2. ZTools Typed Command & Media Parity

- **SPEC-005-R3 (JS Regex Literal Parity):** Typed `regex` and `over.exclude` rules MUST parse `/pattern/flags` literals with ZTools context-specific flag rules (`g`, `i`, `m`, `u`, `y`), UTF-16 length counting, ASCII `\d`/`\D` normalization, and sticky `\A` anchoring, rejecting raw patterns without enclosing slashes.
- **SPEC-005-R4 (Typed Media & File Rule Parity):** Typed `files` rules MUST enforce inclusive `minLength`/`maxLength` counts, `fileType` (`"file"` | `"directory"`), case-insensitive `extensions`, and filename `match` literals/regexes within bounded work limits (`MAX_MEDIA_FILES = 1000`).

## 3. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Implemented in `crates/aitools-core/src/matcher/` and verified:
  - `SPEC-005-R1`: `to_pinyin_initials`, `to_pinyin_full` — verified by `spec_005_r1_pinyin_initials_and_full_coverage` (`crates/aitools-core/src/matcher/mod.rs`).
  - `SPEC-005-R2`: `FeatureSearchIndex` hybrid scoring + `nucleo-matcher` fallback — verified by `spec_005_r2_hybrid_search_ranks_exact_prefix_pinyin_and_fuzzy` (`crates/aitools-core/src/matcher/index.rs`).
  - `SPEC-005-R3`: Typed text/regex contract parity — verified by `spec_005_r3_typed_regex_index_matches_color_hex` and `spec_005_r3_ztools_typed_text_contract_parity` (`crates/aitools-core/tests/parity.rs` against salvaged `launcher-typed-text-contract-cases.json`).
  - `SPEC-005-R4`: Typed media/file contract parity — verified by `spec_005_r4_ztools_media_contract_parity` (`crates/aitools-core/tests/parity.rs` against salvaged `launcher-media-contract-cases.json`).
- **Measured Latency (`wake_latency` bench):** 1,200-feature search measured `P50 = 383 us`, `P99 = 1,798 us`.
