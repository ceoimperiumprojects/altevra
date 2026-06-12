//! Altevra v0.3.4 — Brain Daemon.
//!
//! Long-running tokio scheduler with periodic jobs:
//!   * event_classifier (every 1 min)
//!   * observer_scan (every 5 min)
//!   * vault_indexer (every 15 min)
//!   * insight_synthesizer (every 1 h)
//!   * research_fetcher (every 2 h)
//!   * daily_summary (once/day @ 23:00)
//!   * task_grooming (every 3 h)
//!   * auto_categorizer (every 30 min)
//!
//! Each job records its start/end/error in `brain_jobs`. The scheduler is
//! cooperative: jobs that fail just log and continue; one slow job does not
//! block another. Time-of-day jobs (daily_summary) check on every tick and
//! fire when the wall clock crosses the scheduled boundary.

pub mod backfill;
pub mod connector_sync;
pub mod curator;
pub mod jobs;
pub mod lifecycle;
pub mod notify;
pub mod observer_detectors;
pub mod prompt_tweak;
pub mod resident;
pub mod routing;
pub mod scheduler;
pub mod selfimprove;
pub mod skill_judge;

pub use backfill::{run_observer_backfill, BackfillReport, BACKFILL_SOURCE};
pub use connector_sync::{
    register_connectors_as_tools, run_connector_sync_at, ConnectorSyncReport, ConnectorSyncResult,
};
pub use curator::{curator_digest_line, curate, run_curator, CuratorReport};
pub use jobs::{all_kinds, load_relevance_gate, run_all, JobKind, JobResult};
pub use lifecycle::{
    lifecycle_archive, lifecycle_sweep, LifecycleArchiveReport, LifecycleReport,
    CONTEXT_PACKET_RETENTION_DAYS, PENDING_DELETE_MARKER,
};
pub use prompt_tweak::{
    apply_prompt_tweak, detect_low_quality_modes, parse_tweak_body, propose_prompt_tweak,
    record_rejected, ApplyTweakOutcome, ProposeOutcome, PromptTweakBody, PROMPT_TWEAK_KIND,
    PROMPT_TWEAK_MARKER,
};
pub use resident::{parse_role, ResidentRunReport, ResidentRunner};
pub use routing::role_for_object;
pub use scheduler::{BrainConfig, BrainScheduler, BrainStatus};
pub use selfimprove::{resident_disabled, run_self_improve, ApplyOutcome, SelfImproveReport};
pub use skill_judge::{
    drain_skill_reactions, extract_reaction_window, parse_judge_response, DrainReport,
    JudgeVerdict, OllamaJudge, SuccessJudge, REACTION_WINDOW_K,
};
