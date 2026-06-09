pub mod capability;
pub mod classifier;
pub mod config;
pub mod domain;
pub mod entity;
pub mod envelope;
pub mod errors;
pub mod events;
pub mod lifecycle;
pub mod maintenance;
pub mod mirror;
pub mod observer;
pub mod packet;
pub mod paths;
pub mod presence;
pub mod prompt_registry;
pub mod prompts;
pub mod resident;
pub mod retrieval;
pub mod safety;
pub mod security;
pub mod selfimprove;
pub mod session_context;
pub mod status;
pub mod template;
pub mod time_window;
pub mod tombstone;
pub mod updates;

pub use capability::{Support, TrustLevel};
pub use domain::{Domain, RiskTag};
pub use entity::{
    ascii_fold, detect_mentions, last_contact, mentioned_entity_ids, Entity, EntityDictionary,
    EntityKind, EntityRef,
};
pub use envelope::{Confidence, Envelope, HasEnvelope, Provenance, ProvenanceOrigin};
pub use errors::AltevraError;
pub use lifecycle::derive_lifecycle_state;
pub use maintenance::{
    maintenance_lock_path, maintenance_locked, maintenance_locked_default, spool_dir,
    MaintenanceLock, MAINTENANCE_LOCK_TTL_SECS,
};
pub use mirror::{render_mirror, MirrorDoc};
pub use observer::{
    detect_decision_conflict, detect_high_session_volume, detect_low_task_velocity,
    detect_patterns, detect_recurring_drift, detect_repeated_hook_failure, detect_secret_churn,
    detect_skill_version_divergence, detect_stale_project, EvidenceRef, Insight, InsightKind,
};
pub use packet::{
    ContextPacket, ContextPacketItem, ExclusionRecord, PacketCandidate, PacketCompiler,
    PacketRequest, WhyIncluded,
};
pub use paths::{
    current_session_path, default_brain_pid_path, default_db_path, default_vault_path,
    default_watcher_pid_path, home_dir, DEFAULT_BRAIN_PID, DEFAULT_DB_PATH, DEFAULT_WATCHER_PID,
};
pub use presence::{require_human_presence, PresenceError, PresenceMethod, PresenceProof};
pub use prompt_registry::{
    assert_one_active_per_slug, checksum_body, detect_drift, mint_plan, render, try_auto_activate,
    AutoActivateDecision, DriftFinding, DuplicateActive, MintError, MintPlan, MissingSlug,
    PromptEval, PromptRecord, RenderManifest, RenderManifestEntry, RenderedPrompt,
};
pub use resident::{
    parse_resident_output, ResidentMode, ResidentOutput, ResidentProposal, ResidentRunStatus,
};
pub use safety::{DenyReason, ExposureDecision, ExposureGate, ExposureRequest};
pub use security::Sensitivity;
pub use selfimprove::{
    derive_risk_tier, firewall_check, FirewallDenyReason, FirewallLimits, FirewallState,
    FirewallVerdict, ProposedAction, RiskTier,
};
pub use session_context::{
    render_session_context_block, render_tool_register_block, session_start_transport,
    SessionContextData, SessionStartTransport, ToolSummary, SESSION_BLOCK_TOKEN_BUDGET,
};
pub use status::{
    CapabilityState, LifecycleState, ObjectStatus, ProposalStatus, RedactionStatus, ReviewStatus,
};
pub use template::gate::{GateOutcome, TemplateGate};
pub use template::{Template, TemplateRegistry};
pub use tombstone::{build_tombstone, detect_conflict, ConflictMarker, ConflictSide, Tombstone};
