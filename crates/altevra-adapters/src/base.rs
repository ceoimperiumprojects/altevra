use altevra_hooks::universal::UniversalHook;
use altevra_skills::parser::ParsedSkill;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// A file to be generated/installed by an adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedFile {
    /// Destination path (relative to repo root).
    pub path: PathBuf,
    pub content: String,
    pub managed: bool,
    pub checksum: String,
}

impl GeneratedFile {
    pub fn new(path: impl Into<PathBuf>, content: String) -> Self {
        let content_clone = content.clone();
        let checksum = {
            let mut h = Sha256::new();
            h.update(content_clone.as_bytes());
            hex::encode(h.finalize())
        };
        Self {
            path: path.into(),
            content,
            managed: true,
            checksum,
        }
    }

    /// Prepend the Altevra managed header to the content.
    pub fn with_managed_header(mut self, source: &str, adapter: &str, version: &str) -> Self {
        let header = format!(
            "<!-- ALTEVRA_MANAGED: true -->\n\
             <!-- source: {source} -->\n\
             <!-- generated_by: altevra -->\n\
             <!-- adapter: {adapter} -->\n\
             <!-- version: {version} -->\n\
             <!-- checksum: {checksum} -->\n\
             <!-- generated_at: {now} -->\n\n",
            checksum = self.checksum,
            now = Utc::now().to_rfc3339(),
        );
        self.content = format!("{header}{}", self.content);
        self
    }
}

/// Result of adapter detection in a repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterDetectionResult {
    pub tool_name: String,
    pub detected: bool,
    pub repo_path: Option<PathBuf>,
    pub notes: Vec<String>,
}

/// Input for rendering instructions.
#[derive(Debug, Clone)]
pub struct InstructionRenderInput {
    pub tool_name: String,
    pub project: Option<String>,
    pub repo_path: PathBuf,
    pub altevra_version: String,
}

/// A plan for what would be installed — used for dry-run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPlan {
    pub tool_name: String,
    pub project: Option<String>,
    pub files_to_create: Vec<InstallPlanFile>,
    pub files_to_update: Vec<InstallPlanFile>,
    pub files_drifted: Vec<InstallPlanFile>,
    /// Skills loaded from vault (06-skills/) to be rendered and installed.
    pub skills_to_install: Vec<altevra_skills::parser::ParsedSkill>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPlanFile {
    pub path: PathBuf,
    pub action: String,
    pub managed: bool,
    pub checksum: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallResult {
    pub tool_name: String,
    pub files_created: Vec<PathBuf>,
    pub files_updated: Vec<PathBuf>,
    pub files_skipped: Vec<PathBuf>,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    pub tool_name: String,
    pub all_ok: bool,
    pub issues: Vec<String>,
    pub drifted_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairPlan {
    pub tool_name: String,
    pub actions: Vec<String>,
}

/// Universal adapter interface — every tool integration implements this.
pub trait ToolAdapter: Send + Sync {
    fn tool_name(&self) -> &'static str;
    fn adapter_version(&self) -> &'static str;
    fn detect(&self, repo_path: &Path) -> AdapterDetectionResult;
    fn render_instructions(
        &self,
        input: InstructionRenderInput,
    ) -> anyhow::Result<Vec<GeneratedFile>>;
    fn render_skills(&self, skills: Vec<&ParsedSkill>) -> anyhow::Result<Vec<GeneratedFile>>;
    fn render_hooks(&self, hooks: Vec<&UniversalHook>) -> anyhow::Result<Vec<GeneratedFile>>;
    fn build_install_plan(
        &self,
        repo_path: &Path,
        project: Option<&str>,
    ) -> anyhow::Result<InstallPlan>;
    fn install(&self, plan: &InstallPlan, repo_path: &Path) -> anyhow::Result<InstallResult>;
    fn verify(&self, repo_path: &Path) -> anyhow::Result<VerifyResult>;
    fn repair(&self, repo_path: &Path) -> anyhow::Result<RepairPlan>;
}
