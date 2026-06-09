pub mod capability;
pub mod cursor_edits;
pub mod domain_policy;
pub mod events;
pub mod exposure;
pub mod firewall_state;
pub mod fts;
pub mod hooks;
pub mod improvement_signals;
pub mod installations;
pub mod mentions;
pub mod objects;
pub mod prompts;
pub mod proposals;
pub mod read_state;
pub mod resident;
pub mod sessions;
pub mod skillopt_meta;
pub mod skills;
pub mod tasks;
pub mod tool_records;
pub mod updates;
pub mod wiki;

pub use capability::{
    AdapterDossierRow, AdapterDossiersRepository, CapabilityGrantRow, CapabilityGrantsRepository,
    CapabilityRecordRow, CapabilityRecordsRepository, SkillProposalsRepository,
};
pub use cursor_edits::{CursorEditRow, CursorEditsRepository};
pub use domain_policy::{CloudSync, DomainPolicyRepository, DomainPolicyRow, EmbeddingModelRole};
pub use events::EventsRepository;
pub use exposure::{ExposureAudit, ExposureDecisionsRepository};
pub use firewall_state::FirewallStateRepository;
pub use fts::{FtsHit, FtsRepository, ObjectHit};
pub use hooks::{HookRow, HookRunRow, HooksRepository};
pub use improvement_signals::{
    is_resident_authored, signal_for_session, signal_for_skill_candidate,
    ImprovementSignalsRepository, NewSignal, SignalCluster, SignalRow,
};
pub use installations::{InstallationsRepository, InstalledComponentRow, ToolInstallationRow};
pub use mentions::{MentionEdge, MentionsRepository};
pub use objects::{
    InsightCardRow, InsightCardsRepository, LearningRow, LearningsRepository,
    ObjectIndexRepository, ObjectIndexRow,
};
pub use prompts::PromptsRepository;
pub use proposals::{write_resident_proposals, NewProposal, ProposalRow, ProposalsRepository};
pub use read_state::{ReadStateRepository, UpdateReadState};
pub use resident::ResidentRepository;
pub use sessions::{FileChangeRow, SessionRow, SessionsRepository, TurnRow, TurnSearchHit};
pub use skillopt_meta::{SkilloptMetaRepository, SkilloptMetaRow, SKILLOPT_OUTCOMES};
pub use skills::{SkillRow, SkillsRepository};
pub use tasks::{
    DecisionDueForReview, DecisionIndexEnvelope, DecisionRow, GoalRow, ReviewItemRow, TaskRow,
    TasksRepository,
};
pub use tool_records::{ToolRecordRow, ToolRecordsRepository, TOOL_KINDS, TOOL_STATUSES};
pub use updates::UpdatesRepository;
pub use wiki::{WikiPageLinkRow, WikiPageRow, WikiPagesRepository};
