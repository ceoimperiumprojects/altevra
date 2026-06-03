//! Resident runtime (P0.5, working draft §4, R10/R14/MOD-2).
//!
//! Runs a small single-purpose [`ResidentMode`] against a scoped context packet
//! and produces a schema-validated, review-routed [`ResidentOutput`]. Dry-run /
//! proposal-only: NOTHING is written to the live store (SI-6) — the caller
//! records a `resident_run` row + the output JSON via
//! [`altevra_db::ResidentRepository`].
//!
//! No keys needed: the [`ModelRouter`] resolves every role to the noop provider
//! until Pavle adds API keys, at which point real providers replace the noop with
//! NO contract change ("just add API keys").

use altevra_core::resident::{
    parse_resident_output, ResidentMode, ResidentOutput, ResidentRunStatus,
};
use altevra_llm::{ChatMessage, ChatOpts, ModelRole, ModelRouter};

/// Canonical generic-envelope OUTPUT CONTRACT appended to every mode's system
/// prompt. It is the ONE output shape [`parse_resident_output`] accepts (R10/§4):
/// a `{proposals:[{kind,title,body,evidence_refs}]}` envelope. Each mode's intent
/// maps onto `kind` (insight/wiki/category/memory/skill/prompt/...) — the values
/// `derive_risk_tier` understands. Modes that propose nothing return an empty list.
///
/// [`parse_resident_output`]: altevra_core::resident::parse_resident_output
const GENERIC_OUTPUT_CONTRACT: &str = "OUTPUT CONTRACT (mandatory): \
Respond with ONLY a single JSON object — no prose, no markdown fences — in EXACTLY \
this shape: {\"proposals\":[{\"kind\":\"<kind>\",\"title\":\"<short title>\",\
\"body\":\"<the full sourced reasoning>\",\"evidence_refs\":[\"<object/turn/run id>\"]}]}. \
Emit one proposal per distilled item. Set \"kind\" to one of: insight, wiki, \
category, memory, skill, prompt, person, relationship, improvement — choose the \
kind that matches your mode's intent. Put your reasoning in \"body\" and cite the \
ids of the evidence you used in \"evidence_refs\". If nothing in the packet \
supports a real proposal, return {\"proposals\":[]}.";

/// The outcome of one dry resident run.
#[derive(Debug, Clone)]
pub struct ResidentRunReport {
    pub mode: String,
    pub model_role: String,
    pub provider_id: String,
    pub status: ResidentRunStatus,
    pub output: ResidentOutput,
    pub dry_run: bool,
}

impl ResidentRunReport {
    pub fn proposals_emitted(&self) -> i64 {
        self.output.proposals.len() as i64
    }
}

/// Parse a mode's declared role string into a [`ModelRole`] (unknown → `None`).
pub fn parse_role(s: &str) -> ModelRole {
    match s {
        "cheap_worker" => ModelRole::CheapWorker,
        "strong_reasoner" => ModelRole::StrongReasoner,
        "local_private" => ModelRole::LocalPrivate,
        "embedding" => ModelRole::Embedding,
        "reranker" => ModelRole::Reranker,
        _ => ModelRole::None,
    }
}

pub struct ResidentRunner<'a> {
    router: &'a ModelRouter,
}

impl<'a> ResidentRunner<'a> {
    pub fn new(router: &'a ModelRouter) -> Self {
        Self { router }
    }

    /// Dry-run a mode against an input context (the compiled packet text).
    /// Proposal-only: never writes to the live store. The returned report is what
    /// the caller persists as a `resident_run` row.
    pub async fn run_dry(&self, mode: &ResidentMode, packet_text: &str) -> ResidentRunReport {
        let base = |status, output, provider_id: String| ResidentRunReport {
            mode: mode.name.clone(),
            model_role: mode.model_role.clone(),
            provider_id,
            status,
            output,
            dry_run: true,
        };

        // SI-7 contract: a personal-data mode must route local_private. A
        // violation skips the run (no provider call, no writes).
        if mode.validate_role_ceiling().is_err() || !mode.enabled {
            return base(
                ResidentRunStatus::Skipped,
                ResidentOutput::default(),
                "none".into(),
            );
        }

        let role = parse_role(&mode.model_role);
        let provider = self.router.resolve(role);
        let provider_id = provider.id().to_string();

        // The mode's description is its (small, single-purpose) system prompt; the
        // packet is the scoped input. We append the canonical generic-envelope
        // OUTPUT CONTRACT so a real model emits the one shape the runtime validator
        // ([`parse_resident_output`]) accepts across ALL modes (R10/§4). Without
        // this a model freelances rich markdown/mode-specific JSON that can never
        // pass schema → status stays failed_schema and no proposal ever lands.
        // One user message keeps the seam minimal.
        let system = format!("{}\n\n{}", mode.description.trim(), GENERIC_OUTPUT_CONTRACT);
        let messages = vec![ChatMessage::system(&system), ChatMessage::user(packet_text)];
        let raw = match provider.complete(&messages, &ChatOpts::default()).await {
            Ok(s) => s,
            Err(_) => {
                return base(
                    ResidentRunStatus::FailedSchema,
                    ResidentOutput::default(),
                    provider_id,
                )
            }
        };

        // The noop provider returns a known stub (no model) → schema-valid EMPTY
        // output (a dry-run with no keys proposes nothing). A REAL provider's
        // output is schema-validated; non-conforming text → FailedSchema, zero
        // writes (SI-14).
        if provider_id == "noop" {
            return base(
                ResidentRunStatus::Completed,
                ResidentOutput::default(),
                provider_id,
            );
        }
        match parse_resident_output(&raw) {
            Ok(output) => base(ResidentRunStatus::Completed, output, provider_id),
            Err(_) => base(
                ResidentRunStatus::FailedSchema,
                ResidentOutput::default(),
                provider_id,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_core::security::Sensitivity;
    use altevra_llm::{ChatProvider, NoopProvider};
    use async_trait::async_trait;
    use std::sync::Arc;

    fn mode(name: &str, role: &str, personal: bool) -> ResidentMode {
        ResidentMode {
            name: name.into(),
            description: "do one job".into(),
            model_role: role.into(),
            sensitivity_ceiling: Sensitivity::Internal,
            personal_data_allowed: personal,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn dry_run_with_noop_is_completed_and_empty() {
        let router = ModelRouter::noop();
        let runner = ResidentRunner::new(&router);
        let r = runner
            .run_dry(&mode("memory_curator", "cheap_worker", false), "packet")
            .await;
        assert_eq!(r.status, ResidentRunStatus::Completed);
        assert_eq!(r.provider_id, "noop");
        assert_eq!(r.proposals_emitted(), 0);
        assert!(r.dry_run);
    }

    #[tokio::test]
    async fn personal_mode_routes_local_only() {
        // personal_curator (local_private) resolves to a LOCAL provider (noop is
        // local). Even if a cloud provider were registered for local_private, the
        // router would refuse it (SI-7) — proven in the llm crate.
        let router = ModelRouter::noop();
        let runner = ResidentRunner::new(&router);
        let r = runner
            .run_dry(&mode("personal_curator", "local_private", true), "packet")
            .await;
        assert_eq!(r.status, ResidentRunStatus::Completed);
        assert_eq!(r.provider_id, "noop");
    }

    #[tokio::test]
    async fn si7_violation_is_skipped() {
        // personal data + non-local role → contract violation → skipped, no run.
        let router = ModelRouter::noop();
        let runner = ResidentRunner::new(&router);
        let r = runner
            .run_dry(&mode("bad", "cheap_worker", true), "packet")
            .await;
        assert_eq!(r.status, ResidentRunStatus::Skipped);
    }

    // A fake provider returning non-schema text → FailedSchema, zero proposals (SI-14).
    struct GarbageProvider;
    #[async_trait]
    impl ChatProvider for GarbageProvider {
        fn id(&self) -> &str {
            "garbage"
        }
        fn is_local(&self) -> bool {
            true
        }
        async fn complete(&self, _m: &[ChatMessage], _o: &ChatOpts) -> anyhow::Result<String> {
            Ok("this is not valid resident output json".into())
        }
    }

    #[tokio::test]
    async fn schema_invalid_real_output_fails_with_zero_writes() {
        let _ = NoopProvider; // keep the import meaningful
        let router =
            ModelRouter::noop().with_provider(ModelRole::CheapWorker, Arc::new(GarbageProvider));
        let runner = ResidentRunner::new(&router);
        let r = runner
            .run_dry(&mode("memory_curator", "cheap_worker", false), "packet")
            .await;
        assert_eq!(r.status, ResidentRunStatus::FailedSchema);
        assert_eq!(r.proposals_emitted(), 0);
    }
}
