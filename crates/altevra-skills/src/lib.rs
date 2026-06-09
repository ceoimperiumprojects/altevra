pub mod checksum;
pub mod importer;
pub mod parser;
pub mod registry;
pub mod renderer;
pub mod skill_edits;
pub mod sync;
pub mod version;
pub mod watcher;

pub use importer::{
    default_skill_dirs, group_by_slug, scan_all, scan_external_dir, ExternalSkill, SourceTool,
};
pub use parser::{ParsedSkill, SkillFrontmatter};
pub use registry::{SkillRegistry, SkillRegistryEntry};
pub use skill_edits::{
    apply_edits, fingerprint_edits, protected_ranges, select_edits, targets_protected,
    EditOutcome, SkillEdit, SkippedEdit, DEFAULT_EDIT_BUDGET, SLOW_UPDATE_END, SLOW_UPDATE_START,
};
pub use sync::{apply_plan, build_plan, SkipReason, SyncAction, SyncPlan, SyncResult};
pub use version::SkillVersion;
pub use watcher::{run_one_cycle, watch_loop, CycleReport, WatchConfig};
