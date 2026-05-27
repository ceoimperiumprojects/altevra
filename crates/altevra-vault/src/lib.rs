//! Altevra vault crate.
//!
//! Read/write Obsidian-style markdown vaults composed of canonical section
//! directories (`00-inbox`, `01-projects`, ...) holding markdown files with
//! optional YAML frontmatter.

pub mod frontmatter;
pub mod parser;
pub mod scanner;
pub mod writer;

pub use frontmatter::{parse_frontmatter, serialize_frontmatter, Frontmatter, FrontmatterError};
pub use parser::{parse_document, parse_document_from_str, ParsedDocument, ParserError};
pub use scanner::{list_sections, scan_vault, ScannedFile, ScannerError, VaultSection};
pub use writer::{write_atomic, write_document, WriterError};
