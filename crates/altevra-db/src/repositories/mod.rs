pub mod capability;
pub mod domain_policy;
pub mod events;
pub mod exposure;
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
pub mod skills;
pub mod tasks;
pub mod updates;
pub mod wiki;

pub use capability::{
    CapabilityGrantRow, CapabilityGrantsRepository, CapabilityRecordRow,
    CapabilityRecordsRepository, SkillProposalsRepository,
};
pub use domain_policy::{CloudSync, DomainPolicyRepository, DomainPolicyRow, EmbeddingModelRole};
pub use events::EventsRepository;
pub use exposure::{ExposureAudit, ExposureDecisionsRepository};
pub use fts::{FtsHit, FtsRepository, ObjectHit};
pub use hooks::{HookRow, HookRunRow, HooksRepository};
pub use improvement_signals::{
    is_resident_authored, signal_for_session, ImprovementSignalsRepository, NewSignal,
    SignalCluster, SignalRow,
};
pub use installations::{InstallationsRepository, InstalledComponentRow, ToolInstallationRow};
pub use mentions::{MentionEdge, MentionsRepository};
pub use objects::{
    InsightCardRow, InsightCardsRepository, LearningRow, LearningsRepository,
    ObjectIndexRepository, ObjectIndexRow,
};
pub use prompts::PromptsRepository;
pub use proposals::{
    write_resident_proposals, NewProposal, ProposalRow, ProposalsRepository,
};
pub use read_state::{ReadStateRepository, UpdateReadState};
pub use resident::ResidentRepository;
pub use sessions::{FileChangeRow, SessionRow, SessionsRepository, TurnRow, TurnSearchHit};
pub use skills::{SkillRow, SkillsRepository};
pub use tasks::{
    DecisionDueForReview, DecisionIndexEnvelope, DecisionRow, GoalRow, ReviewItemRow, TaskRow,
    TasksRepository,
};
pub use updates::UpdatesRepository;
pub use wiki::{WikiPageLinkRow, WikiPageRow, WikiPagesRepository};
