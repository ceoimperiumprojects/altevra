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
pub fn render_with_header(
    skill: &ParsedSkill,
    adapter: &str,
    checksum: &str,
    generated_at: &str,
) -> String {
    let header = format!(
        "<!-- ALTEVRA_MANAGED: true -->\n\
         <!-- source: 06-skills/{slug}.md -->\n\
         <!-- generated_by: altevra -->\n\
         <!-- adapter: {adapter} -->\n\
         <!-- version: {version} -->\n\
         <!-- checksum: {checksum} -->\n\
         <!-- generated_at: {generated_at} -->\n",
        slug = skill.frontmatter.slug,
        version = skill.frontmatter.version,
    );
    format!("{header}\n{}", render_plain(skill))
}
