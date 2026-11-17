//! Bounded typed text regex bank with exact length predicates.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, TryLockError},
};

use regex_automata::{meta, nfa::thompson::WhichCaptures, Input, MatchKind, PatternID, PatternSet};

use super::typed_text_metrics::TextMetrics;
use super::typed_text_pattern::{expression, EffectivePattern, TextContext};
use crate::{AitoolsError, Cmd, FeatureEntry};

pub(crate) const TEXT_REGEX_PROGRAM_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const TEXT_REGEX_DFA_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TYPED_TEXT_REGEX_WORK: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
enum Predicate {
    Regex(usize),
    MinLength { minimum: usize, unicode: bool },
}

#[derive(Clone, Debug)]
pub(crate) struct TypedTextRegexBank {
    expressions: HashMap<EffectivePattern, usize>,
    predicates: Vec<Predicate>,
    regex_count: usize,
    compiled: Option<Arc<Mutex<CachedProgram>>>,
}

impl TypedTextRegexBank {
    pub(super) fn build(features: &[FeatureEntry]) -> crate::Result<Self> {
        Self::build_commands(features.iter().flat_map(|feature| &feature.cmds))
    }

    pub(super) fn build_commands<'a>(
        commands: impl Iterator<Item = &'a Cmd>,
    ) -> crate::Result<Self> {
        let mut expressions = super::typed_text_bank_key::expressions(commands);
        let _unsupported = super::typed_text_bank_key::retain_supported(&mut expressions);
        Self::from_expressions(expressions, TEXT_REGEX_PROGRAM_BYTES)
    }

    fn from_expressions(
        expressions: Vec<EffectivePattern>,
        program_bytes: usize,
    ) -> crate::Result<Self> {
        let regexes = expressions
            .iter()
            .filter_map(|p| match p {
                EffectivePattern::Regex(p) => Some(p),
                _ => None,
            })
            .collect::<Vec<_>>();
        let compiled = if regexes.is_empty() {
            None
        } else {
            let program = meta::Regex::builder()
                .configure(
                    meta::Regex::config()
                        .match_kind(MatchKind::All)
                        .utf8_empty(true)
                        .which_captures(WhichCaptures::None)
                        .nfa_size_limit(Some(program_bytes))
                        .hybrid_cache_capacity(TEXT_REGEX_DFA_BYTES / 2),
                )
                .build_many(&regexes)
                .map_err(|_| compile_budget_error())?;

            if program.memory_usage() > program_bytes {
                return Err(compile_budget_error());
            }
            let cache = program.create_cache();
            let mut bank = CachedProgram { program, cache };
            bank.check_cache_budget()?;
            let trace = super::typed_text_prepare::cache_trace(&regexes, MAX_TYPED_TEXT_REGEX_WORK);
            if !trace.is_empty() {
                let mut matched = PatternSet::new(regexes.len());
                bank.program.which_overlapping_matches_with(
                    &mut bank.cache,
                    &Input::new(&trace),
                    &mut matched,
                );
                bank.check_cache_budget()?;
            }
            Some(Arc::new(Mutex::new(bank)))
        };

        let regex_count = regexes.len();
        let mut regex_id = 0;
        let predicates = expressions
            .iter()
            .map(|p| match p {
                EffectivePattern::Regex(_) => {
                    let id = regex_id;
                    regex_id += 1;
                    Predicate::Regex(id)
                }
                EffectivePattern::MinLength { minimum, unicode } => Predicate::MinLength {
                    minimum: *minimum,
                    unicode: *unicode,
                },
            })
            .collect();

        Ok(Self {
            expressions: expressions
                .into_iter()
                .enumerate()
                .map(|(i, p)| (p, i))
                .collect(),
            predicates,
            regex_count,
            compiled,
        })
    }

    pub(super) fn pattern_id(&self, pattern: Option<&str>, context: TextContext) -> Option<usize> {
        self.expressions
            .get(&expression(pattern?, context)?)
            .copied()
    }

    pub(super) fn matches_with_metrics(
        &self,
        text: &str,
    ) -> crate::Result<(Vec<bool>, TextMetrics)> {
        if text.len().saturating_mul(self.regex_count) > MAX_TYPED_TEXT_REGEX_WORK {
            return Err(AitoolsError::BudgetExceeded(
                "TYPED_TEXT_MATCH_WORK_BUDGET exceeded".into(),
            ));
        }
        let matched = if let Some(compiled) = &self.compiled {
            let mut bank = compiled.try_lock().map_err(|error| {
                AitoolsError::Plugin(
                    match error {
                        TryLockError::WouldBlock => "TYPED_TEXT_SEARCH_BUSY",
                        TryLockError::Poisoned(_) => "Typed text matcher lock poisoned",
                    }
                    .into(),
                )
            })?;
            let mut matches = PatternSet::new(self.regex_count);
            let CachedProgram { program, cache } = &mut *bank;
            program.which_overlapping_matches_with(cache, &Input::new(text), &mut matches);
            bank.check_cache_budget()?;
            Some(
                (0..self.regex_count)
                    .map(|i| matches.contains(PatternID::must(i)))
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };

        let metrics = TextMetrics::from_str(text);
        let matches = self
            .predicates
            .iter()
            .map(|p| match p {
                Predicate::Regex(id) => matched.as_ref().is_some_and(|m| m[*id]),
                Predicate::MinLength {
                    minimum,
                    unicode: true,
                } => metrics.scalar_len >= *minimum,
                Predicate::MinLength {
                    minimum,
                    unicode: false,
                } => metrics.utf16_len >= *minimum,
            })
            .collect();
        Ok((matches, metrics))
    }
}

#[derive(Debug)]
struct CachedProgram {
    program: meta::Regex,
    cache: meta::Cache,
}

impl CachedProgram {
    fn check_cache_budget(&mut self) -> crate::Result<()> {
        if self.cache.memory_usage() > TEXT_REGEX_DFA_BYTES {
            self.cache = self.program.create_cache();
            return Err(AitoolsError::BudgetExceeded(
                "TYPED_TEXT_REGEX_CACHE_BUDGET exceeded".into(),
            ));
        }
        Ok(())
    }
}

fn compile_budget_error() -> AitoolsError {
    AitoolsError::BudgetExceeded("TYPED_TEXT_REGEX_COMPILE_BUDGET exceeded".into())
}
