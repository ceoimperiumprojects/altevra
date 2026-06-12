pub mod antigravity;
pub mod base;
pub mod claude_code;
pub mod codex;
pub mod connectors;
pub mod cursor;
pub mod cursor_cli;
pub mod factory;
pub mod hermes;
pub mod hermes_ingest_sh;

pub use antigravity::AntigravityAdapter;
pub use base::{
    AdapterDetectionResult, GeneratedFile, InstallPlan, InstallResult, InstructionRenderInput,
    RepairPlan, ToolAdapter, VerifyResult,
};
pub use claude_code::ClaudeCodeAdapter;
pub use codex::CodexAdapter;
pub use connectors::{
    builtin_connectors, connector_by_name, domain_sensitivity_floor, ingest_items, AuthMode,
    Connector, ConnectorConfig, ConnectorCtx, ConnectorDescriptor, ConnectorHealth, ConnectorItem,
    ConnectorPayload, ConnectorsConfig, IngestOutcome, ItemProvenance,
};
pub use cursor::CursorAdapter;
pub use cursor_cli::{
    collect_edits, collect_plans, default_ai_tracking_db, default_plans_dir, import,
    CursorImportSummary, CursorPlanRow,
};
pub use factory::{render_skill_proposal, FactoryError, FactoryReport};
pub use hermes::HermesAdapter;
