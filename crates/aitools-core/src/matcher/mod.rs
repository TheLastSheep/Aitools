//! Zero-allocation hybrid search, 20,902+ CJK Pinyin, cached JS regex, and typed parity matcher (`SPEC-005`).

use std::collections::HashMap;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use pinyin::ToPinyin;
use regex::{Regex, RegexBuilder};

use crate::models::FeatureEntry;

pub const SCORE_EXACT: i32 = 100;
pub const SCORE_PREFIX: i32 = 90;
pub const SCORE_REGEX: i32 = 80;
pub const SCORE_IMG: i32 = 75;
pub const SCORE_FILES: i32 = 70;
pub const SCORE_OVER: i32 = 60;
pub const SCORE_CONTAINS: i32 = 50;
pub const SCORE_PINYIN: i32 = 30;
pub const SCORE_FUZZY: i32 = 20;

mod index;
mod media_content;
mod media_regex;
mod text_content;
mod typed_text_bank_key;
mod typed_text_digits;
mod typed_text_metrics;
mod typed_text_pattern;
mod typed_text_prepare;
mod typed_text_regex;

pub use index::{FeatureMeta, FeatureSearchIndex};
pub use media_content::{
    validate_files, MediaFile, MAX_MEDIA_FILES, MAX_MEDIA_PATH_BYTES, MAX_MEDIA_PATH_TOTAL_BYTES,
    MAX_MEDIA_RULE_NAME_BYTES, MAX_MEDIA_RULE_NAME_WORK,
};
pub use typed_text_regex::MAX_TYPED_TEXT_REGEX_WORK;

const MAX_REGEX_CACHE_ENTRIES: usize = 2048;

static REGEX_CACHE: Lazy<RwLock<HashMap<String, Option<Regex>>>> =
    Lazy::new(|| RwLock::new(HashMap::with_capacity(256)));

/// Scored match returned by `FeatureSearchIndex`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchResult {
    pub feature_code: String,
    pub plugin_id: String,
    pub score: i32,
    pub match_type: &'static str,
}

/// Convenience one-shot search over features (repeated searches should reuse [`FeatureSearchIndex`]).
///
/// # Panics
///
/// Panics if building or searching the index exceeds `SPEC-005` memory or CPU budgets;
/// use [`search_all_checked`] for untrusted catalogs.
pub fn search_all(features: &[FeatureEntry], query: &str) -> Vec<MatchResult> {
    search_all_checked(features, query)
        .expect("search_all budget exceeded; use search_all_checked for untrusted catalogs")
}

/// Fallible one-shot search enforcing `SPEC-005` memory and CPU budgets.
///
/// # Errors
///
/// Returns [`crate::AitoolsError::BudgetExceeded`] if feature indexing or query matching exceeds
/// `SPEC-005` CPU or memory budgets.
pub fn search_all_checked(
    features: &[FeatureEntry],
    query: &str,
) -> crate::Result<Vec<MatchResult>> {
    FeatureSearchIndex::try_build(features)?.search_checked(query)
}

pub(crate) fn get_or_compile_regex(pattern: &str) -> Option<Regex> {
    if let Some(cached) = REGEX_CACHE.read().get(pattern) {
        return cached.clone();
    }
    let compiled = compile_ztools_regex(pattern);
    let mut cache = REGEX_CACHE.write();
    if cache.len() >= MAX_REGEX_CACHE_ENTRIES {
        cache.clear();
    }
    cache.insert(pattern.to_string(), compiled.clone());
    compiled
}

fn compile_ztools_regex(pattern: &str) -> Option<Regex> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix('/') {
        if let Some(last_slash) = rest.rfind('/') {
            let body = &rest[..last_slash];
            let flags = &rest[last_slash + 1..];
            if flags
                .chars()
                .all(|f| matches!(f, 'i' | 'g' | 'm' | 's' | 'u' | 'y' | 'x'))
            {
                let anchored;
                let body = if flags.contains('y') {
                    anchored = format!(r"\A(?:{body})");
                    anchored.as_str()
                } else {
                    body
                };
                let mut builder = RegexBuilder::new(body);
                builder.case_insensitive(flags.contains('i'));
                builder.multi_line(flags.contains('m'));
                builder.dot_matches_new_line(flags.contains('s'));
                builder.ignore_whitespace(flags.contains('x'));
                if let Ok(re) = builder.build() {
                    return Some(re);
                }
            }
        }
    }

    Regex::new(trimmed).ok()
}

#[inline]
pub(crate) fn match_precomputed_pinyin(
    initials: Option<&str>,
    full_pinyin: Option<&str>,
    query_lower: &str,
) -> bool {
    if let Some(init) = initials {
        if init.starts_with(query_lower) {
            return true;
        }
    }
    if query_lower.len() >= 2 {
        if let Some(full) = full_pinyin {
            if full.starts_with(query_lower) {
                return true;
            }
        }
    }
    false
}

/// Converts Chinese characters to Pinyin initials (`"剪贴板"` -> `"jtb"`).
pub fn to_pinyin_initials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            out.push(pinyin_initial_from_char(c));
        }
    }
    out
}

/// Converts Chinese characters to full toneless Pinyin (`"剪贴板"` -> `"jiantieban"`).
pub fn to_pinyin_full(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else if let Some(py) = c.to_pinyin() {
            out.push_str(py.plain());
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn to_pinyin_initials_lower(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(pinyin_initial_from_char(c).to_ascii_lowercase());
        }
    }
    out
}

pub(crate) fn to_pinyin_full_lower(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
        } else if let Some(py) = c.to_pinyin() {
            out.push_str(py.plain());
        } else {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
        }
    }
    out
}

#[inline]
fn pinyin_initial_from_char(c: char) -> char {
    if let Some(py) = c.to_pinyin() {
        if let Some(first) = py.plain().chars().next() {
            return first;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_005_r1_pinyin_initials_and_full_coverage() {
        assert_eq!(to_pinyin_initials("你好"), "nh");
        assert_eq!(to_pinyin_initials("天气查询"), "tqcx");
        assert_eq!(to_pinyin_initials("剪贴板"), "jtb");
        assert_eq!(to_pinyin_full("剪贴板"), "jiantieban");
        assert_eq!(to_pinyin_initials("Paste剪切板"), "Pastejqb");
    }
}
