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

pub mod jobs;
pub mod lifecycle;
pub mod resident;
pub mod scheduler;
pub mod selfimprove;

pub use jobs::{JobKind, JobResult};
pub use lifecycle::{lifecycle_sweep, LifecycleReport};
pub use resident::{parse_role, ResidentRunReport, ResidentRunner};
pub use scheduler::{BrainConfig, BrainScheduler, BrainStatus};
pub use selfimprove::{resident_disabled, run_self_improve, ApplyOutcome, SelfImproveReport};
