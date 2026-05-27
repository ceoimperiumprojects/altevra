use crate::registry::HookRegistry;
use crate::universal::UniversalHook;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRunContext {
    pub hook_slug: String,
    pub tool_name: String,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRunOutcome {
    pub run_id: Uuid,
    pub hook_slug: String,
    pub tool_name: String,
    pub success: bool,
    pub result: serde_json::Value,
    pub error_message: Option<String>,
    pub duration_ms: u64,
    pub actions_executed: Vec<String>,
    pub created_at: chrono::DateTime<Utc>,
}

pub struct HookRunner<'a> {
    registry: &'a HookRegistry,
}

impl<'a> HookRunner<'a> {
    pub fn new(registry: &'a HookRegistry) -> Self {
        Self { registry }
    }

    pub fn run(&self, ctx: HookRunContext) -> HookRunOutcome {
        let start = Instant::now();
        let run_id = Uuid::new_v4();

        let Some(hook) = self.registry.get(&ctx.hook_slug) else {
            return HookRunOutcome {
                run_id,
                hook_slug: ctx.hook_slug,
                tool_name: ctx.tool_name,
                success: false,
                result: serde_json::json!({}),
                error_message: Some("Hook not found in registry".to_string()),
                duration_ms: start.elapsed().as_millis() as u64,
                actions_executed: vec![],
                created_at: Utc::now(),
            };
        };

        info!(
            hook = %ctx.hook_slug,
            tool = %ctx.tool_name,
            "Running hook"
        );

        let actions_executed = self.execute_actions(hook, &ctx);
        let duration_ms = start.elapsed().as_millis() as u64;

        HookRunOutcome {
            run_id,
            hook_slug: ctx.hook_slug,
            tool_name: ctx.tool_name,
            success: true,
            result: serde_json::json!({
                "hook_type": hook.hook_type.to_string(),
                "actions": &actions_executed,
                "project": ctx.project,
                "session_id": ctx.session_id,
            }),
            error_message: None,
            duration_ms,
            actions_executed,
            created_at: Utc::now(),
        }
    }

    fn execute_actions(&self, hook: &UniversalHook, _ctx: &HookRunContext) -> Vec<String> {
        let mut executed = vec![];
        for action in &hook.actions {
            info!(action = %action, "Executing hook action");
            // Action execution is a skeleton — each action would dispatch to
            // the corresponding Altevra service. For MVP, we log and continue.
            executed.push(action.clone());
        }
        executed
    }
}
