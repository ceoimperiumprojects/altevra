pub mod capability;
pub mod classifier;
pub mod config;
pub mod domain;
pub mod envelope;
pub mod errors;
pub mod events;
pub mod observer;
pub mod packet;
pub mod paths;
pub mod presence;
pub mod prompts;
pub mod resident;
pub mod retrieval;
pub mod safety;
pub mod security;
pub mod selfimprove;
pub mod status;
pub mod template;
pub mod updates;

pub use capability::{Support, TrustLevel};
pub use domain::{Domain, RiskTag};
pub use envelope::{Confidence, Envelope, HasEnvelope, Provenance, ProvenanceOrigin};
pub use errors::AltevraError;
pub use observer::{
    detect_decision_conflict, detect_high_session_volume, detect_low_task_velocity,
    detect_patterns, detect_recurring_drift, detect_repeated_hook_failure, detect_secret_churn,
    detect_skill_version_divergence, detect_stale_project, EvidenceRef, Insight, InsightKind,
};
pub use packet::{
    ContextPacket, ContextPacketItem, ExclusionRecord, PacketCandidate, PacketCompiler,
    PacketRequest, WhyIncluded,
};
pub use paths::{default_db_path, DEFAULT_DB_PATH};
pub use presence::{require_human_presence, PresenceError, PresenceMethod, PresenceProof};
pub use resident::{
    parse_resident_output, ResidentMode, ResidentOutput, ResidentProposal, ResidentRunStatus,
};
pub use safety::{DenyReason, ExposureDecision, ExposureGate, ExposureRequest};
pub use security::Sensitivity;
pub use selfimprove::{
    derive_risk_tier, firewall_check, FirewallDenyReason, FirewallLimits, FirewallState,
    FirewallVerdict, ProposedAction, RiskTier,
};
pub use status::{
    CapabilityState, LifecycleState, ObjectStatus, ProposalStatus, RedactionStatus, ReviewStatus,
};
pub use template::gate::{GateOutcome, TemplateGate};
pub use template::{Template, TemplateRegistry};
