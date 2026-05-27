use crate::version::SkillVersion;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Parsed YAML frontmatter from a skill markdown file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub slug: String,
    pub version: String,
    pub title: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub tools: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Fully parsed skill, ready for use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedSkill {
    pub frontmatter: SkillFrontmatter,
    pub body: String,
    pub raw: String,
}

impl ParsedSkill {
    pub fn version(&self) -> Option<SkillVersion> {
        self.frontmatter.version.parse().ok()
    }

    pub fn slug(&self) -> &str {
        &self.frontmatter.slug
    }
}

/// Parse a skill markdown file.
///
/// Expected format:
/// ```text
/// ---
/// slug: altevra-core
/// version: 0.5.0
/// title: Altevra Core Skill
/// ---
///
/// Skill body here...
/// ```
pub fn parse_skill(content: &str) -> anyhow::Result<ParsedSkill> {
    let re = Regex::new(r"(?s)^---\n(.+?)\n---\n?(.*)")?;
    let caps = re
        .captures(content)
        .ok_or_else(|| anyhow::anyhow!("No YAML frontmatter found in skill file"))?;

    let yaml_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    let body = caps
        .get(2)
        .map(|m| m.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let frontmatter: SkillFrontmatter = serde_yaml::from_str(yaml_str)
        .map_err(|e| anyhow::anyhow!("Failed to parse skill frontmatter: {e}"))?;

    Ok(ParsedSkill {
        frontmatter,
        body,
        raw: content.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SKILL: &str = r#"---
slug: altevra-core
version: 0.5.0
title: Altevra Core Skill
description: Core instructions for using Altevra.
tools:
  - claude-code
tags:
  - core
  - bootstrap
---

# Altevra Core

Use `altevra agent bootstrap` at session start.
"#;

    #[test]
    fn test_parse_skill() {
        let skill = parse_skill(SAMPLE_SKILL).unwrap();
        assert_eq!(skill.slug(), "altevra-core");
        assert_eq!(skill.frontmatter.version, "0.5.0");
        assert_eq!(skill.frontmatter.title, "Altevra Core Skill");
        assert!(skill.body.contains("altevra agent bootstrap"));
    }

    #[test]
    fn test_parse_version() {
        let skill = parse_skill(SAMPLE_SKILL).unwrap();
        let v = skill.version().unwrap();
        assert_eq!(v, "0.5.0".parse().unwrap());
    }
}
