use std::{collections::HashMap, sync::Arc};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use tracing::info_span;

use crate::models::{Cmd, CmdType, FeatureEntry};

use super::{
    match_precomputed_pinyin, media_content, media_regex, text_content, to_pinyin_full_lower,
    to_pinyin_initials_lower, typed_text_pattern, typed_text_regex, MatchResult, SCORE_CONTAINS,
    SCORE_EXACT, SCORE_FUZZY, SCORE_OVER, SCORE_PINYIN, SCORE_PREFIX, SCORE_REGEX,
};

/// Pre-indexed feature search structure for ultra-low-latency repeated searches (`SPEC-005`).
#[derive(Debug, Clone)]
pub struct FeatureSearchIndex {
    pub(super) entries: Vec<IndexedFeature>,
    pub(super) media_regex: media_regex::MediaRegexBank,
    pub(super) text_regex: typed_text_regex::TypedTextRegexBank,
}

#[derive(Debug, Clone)]
pub(super) struct IndexedFeature {
    pub(super) code: String,
    pub(super) plugin_id: String,
    pub(super) label_lower: String,
    pub(super) priority: i32,
    pub(super) cmds: Vec<IndexedCmd>,
}

#[derive(Debug, Clone)]
pub(super) enum IndexedCmd {
    Text {
        lower: String,
        is_ascii: bool,
        pinyin_initials: Option<String>,
        pinyin_full: Option<String>,
    },
    Regex {
        ordinary_pattern: Option<usize>,
        fixed_pattern: Option<usize>,
        min_length: u32,
    },
    Over {
        exclude_pattern: Option<usize>,
        min_length: u32,
        max_length: u32,
    },
    Img,
    Files {
        rule: Arc<media_content::FilesRule>,
        min_length: u32,
        max_length: u32,
    },
    Window,
}

impl FeatureSearchIndex {
    /// Builds a search index for `features`.
    ///
    /// # Panics
    ///
    /// Panics if indexing `features` exceeds `SPEC-005` regex or memory budgets;
    /// use [`FeatureSearchIndex::try_build`] for untrusted catalogs.
    pub fn build(features: &[FeatureEntry]) -> Self {
        Self::try_build(features).expect(
            "FeatureSearchIndex::build budget exceeded; use try_build for untrusted catalogs",
        )
    }

    /// Builds a search index for `features` while enforcing `SPEC-005` budgets.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AitoolsError::BudgetExceeded`] if compiled regex rules or indexed entries
    /// exceed `SPEC-005` budgets.
    pub fn try_build(features: &[FeatureEntry]) -> crate::Result<Self> {
        let text_regex = typed_text_regex::TypedTextRegexBank::build(features)?;
        Self::build_with_text_regex(features, text_regex)
    }

    fn build_with_text_regex(
        features: &[FeatureEntry],
        text_regex: typed_text_regex::TypedTextRegexBank,
    ) -> crate::Result<Self> {
        let mut file_rules = HashMap::new();
        let entries = features
            .iter()
            .map(|feature| {
                let cmds = feature
                    .cmds
                    .iter()
                    .map(|cmd| match cmd {
                        Cmd::Text(text) => {
                            let is_ascii = text.is_ascii();
                            let lower = if is_ascii {
                                text.to_ascii_lowercase()
                            } else {
                                text.to_lowercase()
                            };
                            let (pinyin_initials, pinyin_full) = if !is_ascii
                                && text.chars().any(|c| !c.is_ascii() && c.is_alphabetic())
                            {
                                (
                                    Some(to_pinyin_initials_lower(text)),
                                    Some(to_pinyin_full_lower(text)),
                                )
                            } else {
                                (None, None)
                            };
                            IndexedCmd::Text {
                                lower,
                                is_ascii,
                                pinyin_initials,
                                pinyin_full,
                            }
                        }
                        Cmd::Typed(typed) => match typed.type_ {
                            CmdType::Regex => IndexedCmd::Regex {
                                ordinary_pattern: text_regex.pattern_id(
                                    typed.match_.as_deref(),
                                    typed_text_pattern::TextContext::Ordinary,
                                ),
                                fixed_pattern: text_regex.pattern_id(
                                    typed.match_.as_deref(),
                                    typed_text_pattern::TextContext::FixedText,
                                ),
                                min_length: typed.length.unwrap_or(1),
                            },
                            CmdType::Over => IndexedCmd::Over {
                                exclude_pattern: text_regex.pattern_id(
                                    typed.exclude.as_deref(),
                                    typed_text_pattern::TextContext::Ordinary,
                                ),
                                min_length: typed.length.unwrap_or(1),
                                max_length: typed.max_length.unwrap_or(10_000),
                            },
                            CmdType::Img => IndexedCmd::Img,
                            CmdType::Files => IndexedCmd::Files {
                                rule: media_content::indexed_rule(typed, &mut file_rules),
                                min_length: typed.length.unwrap_or(1),
                                max_length: typed.max_length.unwrap_or(10_000),
                            },
                            CmdType::Window => IndexedCmd::Window,
                        },
                    })
                    .collect();

                IndexedFeature {
                    code: feature.code.clone(),
                    plugin_id: feature.plugin_id.clone(),
                    label_lower: feature.label.to_lowercase(),
                    priority: feature.priority,
                    cmds,
                }
            })
            .collect();

        let media_regex = media_regex::MediaRegexBank::build(file_rules.values());
        Ok(Self {
            entries,
            media_regex,
            text_regex,
        })
    }

    /// Searches indexed features, panicking if `query` exceeds `SPEC-005` budgets.
    ///
    /// # Panics
    ///
    /// Panics if `query` exceeds `SPEC-005` size or regex work budgets; use
    /// [`FeatureSearchIndex::search_checked`] for untrusted input.
    pub fn search(&self, query: &str) -> Vec<MatchResult> {
        self.search_checked(query)
            .expect("FeatureSearchIndex::search budget exceeded; use search_checked")
    }

    /// Searches indexed features using zero-alloc deterministic rules and `nucleo-matcher` fuzzy fallback.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AitoolsError::BudgetExceeded`] if `query` exceeds `SPEC-005` byte or
    /// regex work budgets.
    pub fn search_checked(&self, query: &str) -> crate::Result<Vec<MatchResult>> {
        let _span = info_span!("search_index", query_len = query.len()).entered();
        text_content::validate_text_bytes(query)?;
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let (text_matches, text_metrics) = self.text_regex.matches_with_metrics(query)?;

        let query_lower = if query.is_ascii() {
            query.to_ascii_lowercase()
        } else {
            query.to_lowercase()
        };
        let query_char_len = text_metrics.utf16_len as u32;

        let mut scored: Vec<(i32, MatchResult)> = Vec::new();
        let mut fuzzy_candidates: Vec<&IndexedFeature> = Vec::new();

        for feature in &self.entries {
            let mut best: Option<(i32, &'static str)> = None;
            for cmd in &feature.cmds {
                let matched = match cmd {
                    IndexedCmd::Text {
                        lower,
                        is_ascii,
                        pinyin_initials,
                        pinyin_full,
                    } => {
                        if lower == &query_lower {
                            Some((SCORE_EXACT, "exact"))
                        } else if lower.starts_with(&query_lower) {
                            Some((SCORE_PREFIX, "prefix"))
                        } else if !*is_ascii
                            && match_precomputed_pinyin(
                                pinyin_initials.as_deref(),
                                pinyin_full.as_deref(),
                                &query_lower,
                            )
                        {
                            Some((SCORE_PINYIN, "pinyin"))
                        } else if lower.contains(&query_lower) {
                            Some((SCORE_CONTAINS, "contains"))
                        } else {
                            None
                        }
                    }
                    IndexedCmd::Regex {
                        ordinary_pattern,
                        min_length,
                        ..
                    } => {
                        if query_char_len >= *min_length
                            && ordinary_pattern.is_some_and(|id| text_matches[id])
                        {
                            Some((SCORE_REGEX, "regex"))
                        } else {
                            None
                        }
                    }
                    IndexedCmd::Over {
                        exclude_pattern,
                        min_length,
                        max_length,
                    } => {
                        if query_char_len >= *min_length
                            && query_char_len <= *max_length
                            && !exclude_pattern.is_some_and(|id| text_matches[id])
                        {
                            Some((SCORE_OVER, "over"))
                        } else {
                            None
                        }
                    }
                    IndexedCmd::Img | IndexedCmd::Files { .. } | IndexedCmd::Window => None,
                };

                if let Some((score, match_type)) = matched {
                    if best.is_none_or(|(best_score, _)| score > best_score) {
                        best = Some((score, match_type));
                        if score == SCORE_EXACT {
                            break;
                        }
                    }
                }
            }

            if let Some((score, match_type)) = best {
                scored.push((
                    feature.priority,
                    MatchResult {
                        feature_code: feature.code.clone(),
                        plugin_id: feature.plugin_id.clone(),
                        score,
                        match_type,
                    },
                ));
            } else if query_lower.len() >= 2 {
                fuzzy_candidates.push(feature);
            }
        }

        // SPEC-005-R2: hybrid fuzzy fallback using nucleo-matcher when deterministic hits don't dominate.
        if !fuzzy_candidates.is_empty() {
            let mut matcher = Matcher::new(Config::DEFAULT);
            let pattern = Pattern::parse(&query_lower, CaseMatching::Ignore, Normalization::Smart);
            let mut buf = Vec::new();
            for feature in fuzzy_candidates {
                let mut best_fuzzy: Option<u32> =
                    pattern.score(Utf32Str::new(&feature.label_lower, &mut buf), &mut matcher);
                for cmd in &feature.cmds {
                    if let IndexedCmd::Text { lower, .. } = cmd {
                        if let Some(score) =
                            pattern.score(Utf32Str::new(lower, &mut buf), &mut matcher)
                        {
                            best_fuzzy = Some(best_fuzzy.map_or(score, |prev| prev.max(score)));
                        }
                    }
                }
                if let Some(fuzzy_score) = best_fuzzy {
                    let normalized = SCORE_FUZZY + ((fuzzy_score / 25).min(9) as i32);
                    scored.push((
                        feature.priority,
                        MatchResult {
                            feature_code: feature.code.clone(),
                            plugin_id: feature.plugin_id.clone(),
                            score: normalized,
                            match_type: "fuzzy",
                        },
                    ));
                }
            }
        }

        scored.sort_unstable_by(|(prio_a, a), (prio_b, b)| {
            b.score
                .cmp(&a.score)
                .then_with(|| prio_b.cmp(prio_a))
                .then_with(|| a.feature_code.cmp(&b.feature_code))
        });

        Ok(scored.into_iter().map(|(_, res)| res).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Cmd, CmdType, CmdTyped, FeatureEntry};

    fn feature(code: &str, cmds: Vec<Cmd>, priority: i32) -> FeatureEntry {
        FeatureEntry {
            code: code.to_owned(),
            plugin_id: "test-plugin".to_owned(),
            plugin_name: "Test Plugin".to_owned(),
            label: code.to_owned(),
            icon: None,
            explain: code.to_owned(),
            cmds,
            main_push: false,
            priority,
        }
    }

    #[test]
    fn spec_005_r2_hybrid_search_ranks_exact_prefix_pinyin_and_fuzzy() {
        let features = vec![
            feature("clipboard", vec![Cmd::Text("剪贴板".into())], 10),
            feature("json-fmt", vec![Cmd::Text("json formatter".into())], 20),
            feature("exact-js", vec![Cmd::Text("js".into())], 5),
        ];
        let index = FeatureSearchIndex::build(&features);

        let exact = index.search("js");
        assert_eq!(exact[0].feature_code, "exact-js");
        assert_eq!(exact[0].match_type, "exact");
        assert_eq!(exact[1].feature_code, "json-fmt");
        assert_eq!(exact[1].match_type, "prefix");

        let pinyin = index.search("jtb");
        assert_eq!(pinyin[0].feature_code, "clipboard");
        assert_eq!(pinyin[0].match_type, "pinyin");

        let fuzzy = index.search("jsfmt");
        assert!(fuzzy
            .iter()
            .any(|m| m.feature_code == "json-fmt" && m.match_type == "fuzzy"));
    }

    #[test]
    fn spec_005_r3_typed_regex_index_matches_color_hex() {
        let features = vec![feature(
            "color",
            vec![Cmd::Typed(CmdTyped {
                type_: CmdType::Regex,
                label: None,
                match_: Some(r"/^#[0-9a-f]{6}$/i".into()),
                exclude: None,
                length: None,
                max_length: None,
                file_type: None,
                extensions: None,
            })],
            10,
        )];
        let index = FeatureSearchIndex::build(&features);
        let ordinary_hits = index.search("#ff00aa");
        assert_eq!(ordinary_hits.len(), 1);
        assert_eq!(ordinary_hits[0].match_type, "regex");
        assert!(index.search("#FF00aa").is_empty());

        let fixed_hits = index.search_text_content("#FF00aa").unwrap();
        assert_eq!(fixed_hits.len(), 1);
        assert_eq!(fixed_hits[0].match_type, "regex");
        assert!(index.search("not-hex").is_empty());
    }
}
