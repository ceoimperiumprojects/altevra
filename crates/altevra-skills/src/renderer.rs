use crate::parser::ParsedSkill;

/// Render a skill as plain text for injection into a tool's context.
pub fn render_plain(skill: &ParsedSkill) -> String {
    format!(
        "# {title}\n\nVersion: {version}\n\n{body}",
        title = skill.frontmatter.title,
        version = skill.frontmatter.version,
        body = skill.body
    )
}

/// Render a skill with a managed file header for writing to a tool config.
pub fn render_with_header(skill: &ParsedSkill, adapter: &str, checksum: &str) -> String {
    let header = format!(
        "<!-- ALTEVRA_MANAGED: true -->\n\
         <!-- source: 06-skills/{slug}.md -->\n\
         <!-- generated_by: altevra -->\n\
         <!-- adapter: {adapter} -->\n\
         <!-- version: {version} -->\n\
         <!-- checksum: {checksum} -->\n",
        slug = skill.frontmatter.slug,
        version = skill.frontmatter.version,
    );
    format!("{header}\n{}", render_plain(skill))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_skill;

    fn sample_skill() -> ParsedSkill {
        parse_skill("---\nslug: test-skill\nversion: 1.0.0\ntitle: Test Skill\n---\nBody text.")
            .unwrap()
    }

    #[test]
    fn test_render_with_header_is_deterministic() {
        let skill = sample_skill();
        let a = render_with_header(&skill, "claude-code", "abc123");
        let b = render_with_header(&skill, "claude-code", "abc123");
        assert_eq!(a, b, "render_with_header must be deterministic");
    }

    #[test]
    fn test_render_with_header_no_timestamp() {
        let skill = sample_skill();
        let output = render_with_header(&skill, "claude-code", "abc123");
        assert!(
            !output.contains("generated_at"),
            "header must not contain generated_at"
        );
    }
}
