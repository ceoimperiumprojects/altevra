//! Per-tool session parsers. Each implements `parse_file` which yields
//! `Vec<ImportedSession>`. The orchestrator picks parser based on the
//! `DiscoveryReport`.

pub mod antigravity;
pub mod claude_code;
pub mod codex;
pub mod cursor;
pub mod hermes;
