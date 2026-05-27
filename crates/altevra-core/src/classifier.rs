use crate::events::{Event, EventType};
use crate::updates::{Importance, UpdateFeedItem};

/// Classify an event's importance per ARCHITECTURE_V5 section 19.
pub fn classify(event: &Event) -> Importance {
    match event.event_type {
        EventType::SkillDriftDetected
        | EventType::HookFailed
        | EventType::SecretChanged
        | EventType::ErrorLogged => {
            if event_is_critical(event) {
                Importance::Critical
            } else {
                Importance::High
            }
        }
        EventType::SkillUpdated
        | EventType::SkillInstalled
        | EventType::TaskCompleted
        | EventType::DecisionSaved
        | EventType::GoalUpdated
        | EventType::ProjectStatusChanged
        | EventType::ToolConnected
        | EventType::CapabilityAdded => Importance::High,
        EventType::TaskCreated
        | EventType::TaskUpdated
        | EventType::GoalCreated
        | EventType::ResearchSaved
        | EventType::ResearchSynthesized
        | EventType::InsightCreated
        | EventType::AdapterSynced
        | EventType::ConnectorSynced
        | EventType::HookInstalled
        | EventType::ReviewItemCreated => Importance::Medium,
        EventType::DocumentChanged
        | EventType::DocumentIndexed
        | EventType::ConfigChanged
        | EventType::SessionStarted
        | EventType::SessionEnded => Importance::Low,
        // v0.3 observability events — typically Noise level (high volume),
        // but FileChanged on tracked vault files bumps to Low for visibility.
        EventType::FileChanged => Importance::Low,
        EventType::ToolCallObserved
        | EventType::PromptSent
        | EventType::ResponseReceived
        | EventType::McpCall
        | EventType::AgentThinkingStep => Importance::Noise,
    }
}

fn event_is_critical(event: &Event) -> bool {
    // Critical: secret leak blocked, source-of-truth conflict, MCP unavailable, db migration needed.
    if let Some(level) = event
        .payload
        .get("severity")
        .and_then(serde_json::Value::as_str)
    {
        if level == "critical" || level == "blocker" {
            return true;
        }
    }
    matches!(
        event.event_type,
        EventType::SecretChanged | EventType::SkillDriftDetected
    ) && event
        .payload
        .get("blocked")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Generate (title, short_summary) for an update derived from an event.
pub fn summarize(event: &Event) -> (String, String) {
    let title = if !event.title.is_empty() {
        event.title.clone()
    } else {
        humanize_type(&event.event_type)
    };

    let short_summary = event.summary.clone().unwrap_or_else(|| {
        format!(
            "{} by {} ({})",
            humanize_type(&event.event_type),
            event.actor_type,
            event.source,
        )
    });

    (title, short_summary)
}

fn humanize_type(t: &EventType) -> String {
    let s = t.to_string();
    s.replace('_', " ")
        .split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Optional recommended-action hint per event type.
pub fn recommend_action(event: &Event) -> Option<String> {
    match event.event_type {
        EventType::SkillDriftDetected => Some(
            "Review .claude/skills/ for manual edits; run `altevra skill refresh <slug>` to fix"
                .into(),
        ),
        EventType::HookFailed => Some(
            "Check `.altevra/events/updates.jsonl` and re-run hook with --debug for details".into(),
        ),
        EventType::SecretChanged => {
            Some("Rotate dependent integrations and verify with `altevra doctor`".into())
        }
        EventType::SkillUpdated | EventType::SkillInstalled => {
            Some("Run `altevra skill refresh --all` to propagate to connected tools".into())
        }
        EventType::ProjectStatusChanged => {
            Some("Update agent context: `altevra context --project ... --json`".into())
        }
        _ => None,
    }
}

/// Convert an Event into a fully populated UpdateFeedItem.
pub fn event_to_update(event: &Event) -> UpdateFeedItem {
    let importance = classify(event);
    let (title, short_summary) = summarize(event);
    let recommended = recommend_action(event);

    UpdateFeedItem {
        id: uuid::Uuid::new_v4(),
        event_id: event.id,
        project_id: event.project_id,
        update_type: event.event_type.to_string(),
        importance,
        title,
        short_summary,
        agent_summary: None,
        affected_entities: event
            .entity_id
            .as_ref()
            .map(|id| {
                serde_json::json!([{
                    "type": event.entity_type.clone().unwrap_or_default(),
                    "id": id,
                }])
            })
            .unwrap_or(serde_json::Value::Array(vec![])),
        recommended_agent_action: recommended,
        visible_to_agents: true,
        sensitivity: event.sensitivity.clone(),
        created_at: event.created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::ActorType;

    #[test]
    fn skill_drift_classified_high() {
        let event = Event::new(
            EventType::SkillDriftDetected,
            "drift",
            "test",
            ActorType::System,
        );
        assert_eq!(classify(&event), Importance::High);
    }

    #[test]
    fn skill_drift_with_blocked_payload_is_critical() {
        let event = Event::new(
            EventType::SkillDriftDetected,
            "drift",
            "test",
            ActorType::System,
        )
        .with_payload(serde_json::json!({"blocked": true}));
        assert_eq!(classify(&event), Importance::Critical);
    }

    #[test]
    fn session_started_is_low() {
        let event = Event::new(
            EventType::SessionStarted,
            "session",
            "test",
            ActorType::Hook,
        );
        assert_eq!(classify(&event), Importance::Low);
    }

    #[test]
    fn task_created_is_medium() {
        let event = Event::new(EventType::TaskCreated, "task", "test", ActorType::User);
        assert_eq!(classify(&event), Importance::Medium);
    }

    #[test]
    fn skill_updated_is_high() {
        let event = Event::new(EventType::SkillUpdated, "skill", "test", ActorType::System);
        assert_eq!(classify(&event), Importance::High);
    }

    #[test]
    fn event_to_update_carries_event_id() {
        let event = Event::new(EventType::SessionStarted, "hi", "test", ActorType::Hook);
        let update = event_to_update(&event);
        assert_eq!(update.event_id, event.id);
        assert_eq!(update.importance, Importance::Low);
        assert!(update.recommended_agent_action.is_none());
    }

    #[test]
    fn recommend_action_for_drift() {
        let event = Event::new(
            EventType::SkillDriftDetected,
            "drift",
            "test",
            ActorType::System,
        );
        assert!(recommend_action(&event).is_some());
    }

    #[test]
    fn summarize_uses_title_when_present() {
        let event = Event::new(EventType::TaskCreated, "ship v5", "test", ActorType::User);
        let (title, _) = summarize(&event);
        assert_eq!(title, "ship v5");
    }

    #[test]
    fn humanize_type_capitalizes() {
        assert_eq!(
            humanize_type(&EventType::SkillDriftDetected),
            "Skill Drift Detected"
        );
    }

    #[test]
    fn critical_payload_severity_promotes() {
        let event = Event::new(EventType::ErrorLogged, "boom", "test", ActorType::System)
            .with_payload(serde_json::json!({"severity": "critical"}));
        assert_eq!(classify(&event), Importance::Critical);
    }
}
