pub mod classifier;
pub mod config;
pub mod errors;
pub mod events;
pub mod observer;
pub mod paths;
pub mod prompts;
pub mod retrieval;
pub mod security;
pub mod updates;

pub use errors::AltevraError;
pub use observer::{
    detect_decision_conflict, detect_high_session_volume, detect_low_task_velocity,
    detect_patterns, detect_recurring_drift, detect_repeated_hook_failure, detect_secret_churn,
    detect_skill_version_divergence, detect_stale_project, EvidenceRef, Insight, InsightKind,
};
pub use paths::{default_db_path, DEFAULT_DB_PATH};
pub use security::Sensitivity;
