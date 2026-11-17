//! Runtime `SKILL.md` parser and 4,000-token system prompt injector (`SPEC-009-R5`).

use serde::{Deserialize, Serialize};

use crate::{AiError, Result};

pub const MAX_SKILL_TOKENS: usize = 4_000;
const APPROX_CHARS_PER_TOKEN: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDocument {
    pub name: String,
    pub description: String,
    pub body: String,
}

impl SkillDocument {
    /// Parses a `SKILL.md` file with YAML frontmatter (`name`, `description`) + markdown body.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::Skill`] if YAML frontmatter delimiters or required `name` /
    /// `description` fields are missing.
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim_start_matches('\u{feff}');
        let rest = trimmed
            .strip_prefix("---")
            .ok_or_else(|| AiError::Skill("missing opening YAML frontmatter '---'".into()))?;
        let end_idx = rest
            .find("\n---")
            .ok_or_else(|| AiError::Skill("missing closing YAML frontmatter '---'".into()))?;

        let frontmatter = &rest[..end_idx];
        let body = rest[end_idx + 4..].trim().to_string();

        let mut name = None;
        let mut description = None;
        for line in frontmatter.lines() {
            if let Some((key, val)) = line.split_once(':') {
                let key = key.trim();
                let val = val.trim().trim_matches('"').trim_matches('\'').to_string();
                match key {
                    "name" if !val.is_empty() => name = Some(val),
                    "description" if !val.is_empty() => description = Some(val),
                    _ => {}
                }
            }
        }

        let name = name.ok_or_else(|| AiError::Skill("frontmatter missing 'name'".into()))?;
        let description = description
            .ok_or_else(|| AiError::Skill("frontmatter missing 'description'".into()))?;

        Ok(Self {
            name,
            description,
            body,
        })
    }

    /// Formats the skill body for system prompt injection, capped at 4,000 tokens (`SPEC-009-R5`).
    pub fn render_for_system_prompt(&self) -> String {
        let max_chars = MAX_SKILL_TOKENS * APPROX_CHARS_PER_TOKEN;
        let body = if self.body.chars().count() <= max_chars {
            self.body.clone()
        } else {
            let truncated: String = self.body.chars().take(max_chars).collect();
            format!("{truncated}\n...[SKILL.md truncated at 4000-token cap]")
        };
        format!("<skill name=\"{}\">\n{}\n</skill>", self.name, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_009_r5_parses_skill_md_and_caps_at_4000_tokens() {
        let big_body = "x".repeat(20_000);
        let raw = format!("---\nname: pdf-helper\ndescription: Extracts PDFs\n---\n{big_body}");
        let doc = SkillDocument::parse(&raw).unwrap();
        assert_eq!(doc.name, "pdf-helper");
        let rendered = doc.render_for_system_prompt();
        assert!(rendered.contains("truncated at 4000-token cap"));
        assert!(rendered.len() < 16_500);
    }
}
