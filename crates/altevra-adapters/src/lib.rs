pub mod antigravity;
pub mod base;
pub mod claude_code;
pub mod codex;
pub mod cursor;
pub mod cursor_cli;
pub mod factory;
pub mod hermes;

pub use antigravity::AntigravityAdapter;
pub use base::{
    AdapterDetectionResult, GeneratedFile, InstallPlan, InstallResult, InstructionRenderInput,
    RepairPlan, ToolAdapter, VerifyResult,
};
pub use claude_code::ClaudeCodeAdapter;
pub use codex::CodexAdapter;
pub use cursor::CursorAdapter;
pub use cursor_cli::{
    collect_edits, collect_plans, default_ai_tracking_db, default_plans_dir, import,
    CursorImportSummary, CursorPlanRow,
};
pub use factory::{render_skill_proposal, FactoryError, FactoryReport};
pub use hermes::HermesAdapter;
