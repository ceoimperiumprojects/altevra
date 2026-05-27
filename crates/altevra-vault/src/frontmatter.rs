//! YAML frontmatter parsing for Obsidian-style markdown files.
//!
//! Frontmatter is a YAML block delimited by `---` at the very top of a markdown
//! file:
//!
//! ```markdown
//! ---
//! title: My Note
//! tags: [foo, bar]
//! ---
//!
//! # Body starts here
//! ```

use thiserror::Error;

/// Errors returned from frontmatter parsing/serialization.
#[derive(Debug, Error)]
pub enum FrontmatterError {
    #[error("malformed frontmatter: missing closing `---` delimiter")]
    MissingClosingDelimiter,

    #[error("invalid YAML in frontmatter: {0}")]
    InvalidYaml(#[from] serde_yaml::Error),
}

/// Parsed YAML frontmatter block.
#[derive(Debug, Clone, PartialEq)]
pub struct Frontmatter {
    /// Arbitrary YAML value (typically a mapping).
    pub raw: serde_yaml::Value,
}

impl Frontmatter {
    /// Build a frontmatter wrapper from an arbitrary YAML value.
    pub fn new(raw: serde_yaml::Value) -> Self {
        Self { raw }
    }

    /// Convenience: get a string field from a mapping-shaped frontmatter.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.raw.get(key).and_then(|v| v.as_str())
    }

    /// Convenience: get tags as Vec<String> from either an array or a comma
    /// separated string.
    pub fn tags(&self) -> Vec<String> {
        match self.raw.get("tags") {
            Some(serde_yaml::Value::Sequence(seq)) => seq
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect(),
            Some(serde_yaml::Value::String(s)) => s
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// Parse the leading `--- ... ---` YAML block from a markdown document.
///
/// Returns `(None, content)` if the document does not start with a frontmatter
/// block.  Returns an error if a block is started but not closed, or if the
/// YAML inside is malformed.
pub fn parse_frontmatter(content: &str) -> anyhow::Result<(Option<Frontmatter>, &str)> {
    // Frontmatter must start at the very first byte with "---" followed by a
    // newline.  Accept either LF or CRLF line endings.
    let rest = if let Some(rest) = content.strip_prefix("---\n") {
        rest
    } else if let Some(rest) = content.strip_prefix("---\r\n") {
        rest
    } else {
        return Ok((None, content));
    };

    // Find the closing "---" delimiter on its own line.
    let mut search_from = 0usize;
    let bytes = rest.as_bytes();
    let close_pos = loop {
        // Look for "\n---" anchored at a line start.
        let Some(pos) = find_line_delimiter(bytes, search_from) else {
            return Err(FrontmatterError::MissingClosingDelimiter.into());
        };
        // pos points to the '-' of "---" at the start of a line.
        let after = pos + 3;
        // The delimiter is valid if it's followed by EOF, '\n', or '\r\n'.
        let valid_close = after == bytes.len()
            || bytes[after] == b'\n'
            || (bytes[after] == b'\r' && bytes.get(after + 1) == Some(&b'\n'));
        if valid_close {
            break pos;
        }
        search_from = pos + 1;
    };

    let yaml_str = &rest[..close_pos];
    // Trim a trailing newline from the YAML chunk if any (line before `---`).
    let yaml_str = yaml_str.trim_end_matches(['\n', '\r']);

    // Compute the body slice: skip the closing "---" and any following newline.
    let mut body_start = close_pos + 3;
    if rest.as_bytes().get(body_start) == Some(&b'\r') {
        body_start += 1;
    }
    if rest.as_bytes().get(body_start) == Some(&b'\n') {
        body_start += 1;
    }
    let body = &rest[body_start.min(rest.len())..];

    // Empty frontmatter (just `---\n---\n`) maps to Null.
    let value: serde_yaml::Value = if yaml_str.trim().is_empty() {
        serde_yaml::Value::Null
    } else {
        serde_yaml::from_str(yaml_str).map_err(FrontmatterError::InvalidYaml)?
    };

    Ok((Some(Frontmatter { raw: value }), body))
}

/// Serialize a [`Frontmatter`] back into the `--- ... ---\n` block (including
/// trailing newline after the closing delimiter).
pub fn serialize_frontmatter(fm: &Frontmatter) -> anyhow::Result<String> {
    let yaml = serde_yaml::to_string(&fm.raw).map_err(FrontmatterError::InvalidYaml)?;
    // serde_yaml emits a trailing newline; normalize.
    let yaml = yaml.trim_end_matches('\n');
    Ok(format!("---\n{yaml}\n---\n"))
}

/// Find a position `i` such that `bytes[i..i+3] == b"---"` and `i` starts a new
/// line (i.e. either i == 0, or bytes[i-1] == b'\n').  Returns None if not
/// found.  `search_from` is the byte index to begin scanning from.
fn find_line_delimiter(bytes: &[u8], search_from: usize) -> Option<usize> {
    let mut i = search_from;
    while i + 3 <= bytes.len() {
        if &bytes[i..i + 3] == b"---" && (i == 0 || bytes[i - 1] == b'\n') {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_frontmatter() {
        let input = "---\ntitle: Hello\ntags: [a, b]\n---\n# Heading\nBody text\n";
        let (fm, body) = parse_frontmatter(input).unwrap();
        let fm = fm.expect("frontmatter present");
        assert_eq!(fm.get_str("title"), Some("Hello"));
        assert_eq!(fm.tags(), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(body, "# Heading\nBody text\n");
    }

    #[test]
    fn missing_frontmatter_returns_none() {
        let input = "# No frontmatter here\n";
        let (fm, body) = parse_frontmatter(input).unwrap();
        assert!(fm.is_none());
        assert_eq!(body, input);
    }

    #[test]
    fn empty_input_returns_none() {
        let input = "";
        let (fm, body) = parse_frontmatter(input).unwrap();
        assert!(fm.is_none());
        assert_eq!(body, "");
    }

    #[test]
    fn missing_closing_delimiter_errors() {
        let input = "---\ntitle: oops\nno closing\n";
        let err = parse_frontmatter(input).unwrap_err();
        assert!(err.to_string().contains("closing"));
    }

    #[test]
    fn malformed_yaml_errors() {
        // `:` at start of key makes invalid YAML.
        let input = "---\n: : :\n\tbad\n---\nbody\n";
        assert!(parse_frontmatter(input).is_err());
    }

    #[test]
    fn empty_frontmatter_block_is_null() {
        let input = "---\n---\nbody\n";
        let (fm, body) = parse_frontmatter(input).unwrap();
        let fm = fm.unwrap();
        assert!(matches!(fm.raw, serde_yaml::Value::Null));
        assert_eq!(body, "body\n");
    }

    #[test]
    fn comma_separated_tags_string_is_parsed() {
        let input = "---\ntags: \"alpha, beta, gamma\"\n---\nbody";
        let (fm, _) = parse_frontmatter(input).unwrap();
        let fm = fm.unwrap();
        assert_eq!(fm.tags(), vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn serialize_roundtrips() {
        let mut map = serde_yaml::Mapping::new();
        map.insert(
            serde_yaml::Value::String("title".into()),
            serde_yaml::Value::String("Hi".into()),
        );
        let fm = Frontmatter::new(serde_yaml::Value::Mapping(map));
        let s = serialize_frontmatter(&fm).unwrap();
        assert!(s.starts_with("---\n"));
        assert!(s.trim_end().ends_with("---"));
        // Re-parse with body attached.
        let combined = format!("{s}body\n");
        let (parsed, body) = parse_frontmatter(&combined).unwrap();
        assert_eq!(parsed.unwrap().get_str("title"), Some("Hi"));
        assert_eq!(body, "body\n");
    }

    #[test]
    fn crlf_line_endings_are_supported() {
        let input = "---\r\ntitle: CRLF\r\n---\r\nbody\r\n";
        let (fm, body) = parse_frontmatter(input).unwrap();
        let fm = fm.unwrap();
        assert_eq!(fm.get_str("title"), Some("CRLF"));
        assert_eq!(body, "body\r\n");
    }
}
