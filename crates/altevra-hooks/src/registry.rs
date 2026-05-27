use crate::universal::{UniversalHook, UniversalHookType};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct HookRegistry {
    hooks: HashMap<String, UniversalHook>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load built-in default hooks (session_start, session_end).
    pub fn with_defaults() -> Self {
        let mut r = Self::new();
        r.register(UniversalHook {
            slug: "session_start".to_string(),
            version: "0.1.0".to_string(),
            hook_type: UniversalHookType::SessionStart,
            actions: vec![
                "check_skill_version".to_string(),
                "get_last_updates".to_string(),
                "get_project_context".to_string(),
                "start_session_log".to_string(),
            ],
            description: Some("Bootstrap agent context at session start.".to_string()),
            enabled: true,
        });
        r.register(UniversalHook {
            slug: "session_end".to_string(),
            version: "0.1.0".to_string(),
            hook_type: UniversalHookType::SessionEnd,
            actions: vec!["end_session_log".to_string(), "emit_event".to_string()],
            description: Some("Finalize session log and emit session_ended event.".to_string()),
            enabled: true,
        });
        r.register(UniversalHook {
            slug: "on_error".to_string(),
            version: "0.1.0".to_string(),
            hook_type: UniversalHookType::OnError,
            actions: vec!["emit_event".to_string(), "create_review_item".to_string()],
            description: Some("Log errors as events and create review items.".to_string()),
            enabled: true,
        });
        r
    }

    pub fn register(&mut self, hook: UniversalHook) {
        self.hooks.insert(hook.slug.clone(), hook);
    }

    pub fn get(&self, slug: &str) -> Option<&UniversalHook> {
        self.hooks.get(slug)
    }

    pub fn list(&self) -> Vec<&UniversalHook> {
        let mut hooks: Vec<_> = self.hooks.values().collect();
        hooks.sort_by(|a, b| a.slug.cmp(&b.slug));
        hooks
    }

    pub fn list_by_type(&self, hook_type: &UniversalHookType) -> Vec<&UniversalHook> {
        self.hooks
            .values()
            .filter(|h| &h.hook_type == hook_type && h.enabled)
            .collect()
    }
}
