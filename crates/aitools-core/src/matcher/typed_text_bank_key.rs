//! Canonical expression sets independent of feature identity and presentation.

use super::typed_text_pattern::{expression, EffectivePattern, TextContext};
use crate::{Cmd, CmdType};

pub(super) fn expressions<'a>(commands: impl Iterator<Item = &'a Cmd>) -> Vec<EffectivePattern> {
    let mut expressions = Vec::new();
    for command in commands {
        let Cmd::Typed(rule) = command else { continue };
        match rule.type_ {
            CmdType::Regex => {
                if let Some(pattern) = &rule.match_ {
                    expressions.extend(
                        [TextContext::Ordinary, TextContext::FixedText]
                            .into_iter()
                            .filter_map(|mode| expression(pattern, mode)),
                    );
                }
            }
            CmdType::Over => {
                if let Some(pattern) = &rule.exclude {
                    expressions.extend(expression(pattern, TextContext::Ordinary));
                }
            }
            _ => {}
        }
    }
    expressions.sort_unstable();
    expressions.dedup();
    expressions
}

pub(super) fn retain_supported(expressions: &mut Vec<EffectivePattern>) -> Vec<EffectivePattern> {
    let mut unsupported = Vec::new();
    expressions.retain(|pattern| {
        let supported = match pattern {
            EffectivePattern::Regex(pattern) => regex_syntax::Parser::new().parse(pattern).is_ok(),
            EffectivePattern::MinLength { .. } => true,
        };
        if !supported {
            unsupported.push(pattern.clone());
        }
        supported
    });
    unsupported
}
