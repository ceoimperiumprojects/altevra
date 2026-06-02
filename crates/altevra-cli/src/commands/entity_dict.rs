//! Entity dictionary loader — now a thin re-export of `altevra_vault::entity_dict`.
//!
//! The loader moved into `altevra-vault` (which already has serde_yaml + reads the
//! vault) so BOTH the CLI capture path AND the MCP `recall_about` tool build the
//! same dictionary — the entity graph is reachable by every AI tool, not just the
//! terminal. This shim keeps the existing `crate::commands::entity_dict::*` call
//! sites stable.

pub use altevra_vault::entity_dict::build_dictionary;
