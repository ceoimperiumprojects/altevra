//! Hermes → Altevra ingest bridge script.
//!
//! Hermes' `shell_hooks.py` can invoke external commands via subprocess, piping
//! the hook payload as JSON on stdin and setting `HOOK_EVENT_NAME` in the
//! environment. This module exposes the bash bridge content that maps Hermes
//! event names to Altevra event names and forwards the payload to
//! `altevra hook-handle`.
//!
//! Design constraints:
//! * Bridge MUST exit 0 unconditionally — never block Hermes on an Altevra
//!   failure (sibling-agent, not gate).
//! * Event names are mapped explicitly; unknown events are silently ignored.
//! * `pre_llm_call` is intentionally a no-op (Altevra captures user prompts via
//!   `user_prompt_submit`, fed by Hermes' `post_llm_call`).
//! * Output identifies the source tool as `hermes` for downstream filtering.

/// Hermes → Altevra event-name mapping table.
///
/// Returns `Some(altevra_event)` if the Hermes event has a matching Altevra
/// handler, `None` if it should be skipped (no-op exit 0).
pub fn map_hermes_event(hermes_event: &str) -> Option<&'static str> {
    match hermes_event {
        "pre_tool_call" => Some("pre_tool_use"),
        "post_tool_call" => Some("post_tool_use"),
        "session_start" => Some("session_start"),
        "session_end" => Some("session_end"),
        // post_llm_call carries the assistant turn that just completed; we treat
        // it as a user-prompt-submit signal so the upcoming turn is tagged.
        "post_llm_call" => Some("user_prompt_submit"),
        // pre_llm_call fires before LLM dispatch — no useful turn payload yet.
        "pre_llm_call" => None,
        _ => None,
    }
}

/// Bash script content rendered to `hooks/altevra-ingest.sh` inside the Hermes
/// base directory (`~/.imperium`). Hermes' `shell_hooks.py` calls this script
/// with the hook payload on stdin and `HOOK_EVENT_NAME` set.
pub fn altevra_ingest_sh_content() -> &'static str {
    ALTEVRA_INGEST_SH
}

const ALTEVRA_INGEST_SH: &str = r#"#!/usr/bin/env bash
# ALTEVRA_MANAGED: true
# source: hermes-adapter
# generated_by: altevra
# adapter: hermes
#
# Hermes → Altevra hook bridge.
#
# Hermes shell_hooks.py invokes external scripts with:
#   * HOOK_EVENT_NAME env var (e.g. pre_tool_call, post_tool_call,
#     session_start, session_end, pre_llm_call, post_llm_call)
#   * Hook payload as JSON on stdin
#
# This script maps Hermes event names → Altevra event names and forwards the
# payload to `altevra hook-handle`. It ALWAYS exits 0 so Altevra failures never
# block Hermes (sibling agent, not gate).

set +e  # never abort on subcommand failure
set +u  # tolerate unset HOOK_EVENT_NAME

# 1) Read stdin payload once (Hermes pipes JSON in).
STDIN_JSON=$(cat || true)

# 2) Resolve Hermes event name: HOOK_EVENT_NAME env wins; fall back to JSON
#    field "event" / "hook_event_name" if the env var is missing.
HERMES_EVENT="${HOOK_EVENT_NAME:-}"
if [ -z "$HERMES_EVENT" ] && [ -n "$STDIN_JSON" ] && command -v jq >/dev/null 2>&1; then
    HERMES_EVENT=$(printf '%s' "$STDIN_JSON" \
        | jq -r '.event // .hook_event_name // empty' 2>/dev/null)
fi

if [ -z "$HERMES_EVENT" ]; then
    exit 0
fi

# 3) Map Hermes → Altevra event names.
case "$HERMES_EVENT" in
    pre_tool_call)   ALTEVRA_EVENT="pre_tool_use" ;;
    post_tool_call)  ALTEVRA_EVENT="post_tool_use" ;;
    session_start)   ALTEVRA_EVENT="session_start" ;;
    session_end)     ALTEVRA_EVENT="session_end" ;;
    post_llm_call)   ALTEVRA_EVENT="user_prompt_submit" ;;
    pre_llm_call)    exit 0 ;;  # no-op: no payload to record yet
    *)               exit 0 ;;  # unknown event: silently skip
esac

# 4) Forward payload to altevra. Never let a missing binary or failure leak.
if command -v altevra >/dev/null 2>&1; then
    printf '%s' "$STDIN_JSON" \
        | altevra hook-handle "$ALTEVRA_EVENT" --tool hermes >/dev/null 2>&1 || true
fi

exit 0
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn altevra_ingest_sh_maps_event_names_correctly() {
        assert_eq!(map_hermes_event("pre_tool_call"), Some("pre_tool_use"));
        assert_eq!(map_hermes_event("post_tool_call"), Some("post_tool_use"));
        assert_eq!(map_hermes_event("session_start"), Some("session_start"));
        assert_eq!(map_hermes_event("session_end"), Some("session_end"));
        assert_eq!(
            map_hermes_event("post_llm_call"),
            Some("user_prompt_submit")
        );
        // pre_llm_call is intentionally a no-op.
        assert_eq!(map_hermes_event("pre_llm_call"), None);
        // Unknown events are silently skipped.
        assert_eq!(map_hermes_event("garbage"), None);
        assert_eq!(map_hermes_event(""), None);
    }

    #[test]
    fn script_contains_case_arms_for_every_mapped_event() {
        let body = altevra_ingest_sh_content();
        // Every mapped Hermes event must appear as a case arm in the script.
        for hermes_event in [
            "pre_tool_call",
            "post_tool_call",
            "session_start",
            "session_end",
            "post_llm_call",
            "pre_llm_call",
        ] {
            assert!(
                body.contains(hermes_event),
                "script must reference Hermes event `{hermes_event}`"
            );
        }
        // And every mapped Altevra event must appear as an assigned value.
        for altevra_event in [
            "pre_tool_use",
            "post_tool_use",
            "session_start",
            "session_end",
            "user_prompt_submit",
        ] {
            assert!(
                body.contains(altevra_event),
                "script must reference Altevra event `{altevra_event}`"
            );
        }
    }
}
