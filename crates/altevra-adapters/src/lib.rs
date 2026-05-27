pub mod antigravity;
pub mod base;
pub mod claude_code;
pub mod codex;
pub mod cursor;

pub use antigravity::AntigravityAdapter;
pub use base::{
    AdapterDetectionResult, GeneratedFile, InstallPlan, InstallResult, InstructionRenderInput,
    RepairPlan, ToolAdapter, VerifyResult,
};
pub use claude_code::ClaudeCodeAdapter;
pub use codex::CodexAdapter;
pub use cursor::CursorAdapter;
