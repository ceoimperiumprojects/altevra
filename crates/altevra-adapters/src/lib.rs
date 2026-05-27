pub mod base;
pub mod claude_code;

pub use base::{
    AdapterDetectionResult, GeneratedFile, InstallPlan, InstallResult, InstructionRenderInput,
    RepairPlan, ToolAdapter, VerifyResult,
};
pub use claude_code::ClaudeCodeAdapter;
