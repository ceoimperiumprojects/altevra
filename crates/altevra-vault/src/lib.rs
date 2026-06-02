//! Altevra vault crate.
//!
//! Read/write Obsidian-style markdown vaults composed of canonical section
//! directories (`00-inbox`, `01-projects`, ...) holding markdown files with
//! optional YAML frontmatter.

pub mod frontmatter;
pub mod normalize;
pub mod parser;
pub mod scanner;
pub mod section_template;
pub mod sections;
pub mod wiki;
pub mod writer;

pub use frontmatter::{parse_frontmatter, serialize_frontmatter, Frontmatter, FrontmatterError};
pub use normalize::{
    classify_path, normalize_frontmatter, render_normalized, split_for_normalize, DocClass,
    UNIVERSAL_KEYS,
};
pub use parser::{parse_document, parse_document_from_str, ParsedDocument, ParserError};
pub use scanner::{list_sections, scan_vault, ScannedFile, ScannerError, VaultSection};
pub use section_template::{
    build_rewrite_prompt, contract_for, scaffold_section, section_conformance, LabelSlot,
    RewritePrompt, SectionConformance, SectionContract,
};
pub use sections::{parse_sections, Section};
pub use wiki::{
    extract_wiki_links, list_wiki_pages, parse_wiki_page, WikiConfidence, WikiError, WikiPage,
    WikiStatus,
};
pub use writer::{write_atomic, write_document, WriterError};
