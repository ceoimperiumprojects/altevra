//! Human-presence authentication (RECONCILIATION R4, invariants HP-1 / HP-2).
//!
//! Every protected approval (review approve/reject, connect, grant approve,
//! forget --execute, domain set-policy, legal-hold, export --raw) must prove a
//! HUMAN is present — an agent must never be able to forge approval.
//!
//! P0 mechanism: TTY presence (`std::io::IsTerminal`) OR a one-shot
//! `ALTEVRA_UNLOCK` env token (for legitimate non-interactive Pavle, e.g. via
//! Hermes). Absent both → refused.
//!
//! - HP-1: no MCP/agent caller has any approve/apply path — enforced by the MCP
//!   server simply NOT exposing these verbs; this module is only ever called
//!   from the human-facing CLI.
//! - HP-2: `"approved"` is never an accepted input field; approval is recorded
//!   by core AFTER a [`require_human_presence`] check, with the method captured.

use std::io::IsTerminal;

/// How human presence was established (recorded in the audit, never a payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceMethod {
    /// Interactive terminal (stdin is a TTY).
    Tty,
    /// One-shot `ALTEVRA_UNLOCK` token supplied by Pavle.
    UnlockToken,
}

/// Proof that a human authorized a protected action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceProof {
    pub method: PresenceMethod,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceError {
    /// No TTY and no unlock token — an agent/non-interactive caller.
    RequiresHumanPresence,
}

impl std::fmt::Display for PresenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresenceError::RequiresHumanPresence => write!(
                f,
                "requires_human_presence: this action needs an interactive terminal \
                 or a valid ALTEVRA_UNLOCK token; agents may only propose, never approve"
            ),
        }
    }
}

impl std::error::Error for PresenceError {}

const UNLOCK_ENV: &str = "ALTEVRA_UNLOCK";

/// Require human presence for a protected action.
///
/// Returns a [`PresenceProof`] (TTY or unlock token) or refuses. Pure-ish:
/// reads stdin TTY state + the unlock env. Designed to be the ONLY gate
/// protected CLI verbs call before recording an approval.
pub fn require_human_presence() -> Result<PresenceProof, PresenceError> {
    if std::io::stdin().is_terminal() {
        return Ok(PresenceProof {
            method: PresenceMethod::Tty,
        });
    }
    // One-shot token path for non-interactive Pavle (e.g. driving from Hermes).
    if let Ok(tok) = std::env::var(UNLOCK_ENV) {
        if !tok.trim().is_empty() {
            return Ok(PresenceProof {
                method: PresenceMethod::UnlockToken,
            });
        }
    }
    Err(PresenceError::RequiresHumanPresence)
}

/// Testable core of the presence check, decoupled from the real environment so
/// the policy itself is unit-tested (the live `require_human_presence` wires
/// this to stdin + env).
pub fn presence_from(
    is_tty: bool,
    unlock_token: Option<&str>,
) -> Result<PresenceProof, PresenceError> {
    if is_tty {
        return Ok(PresenceProof {
            method: PresenceMethod::Tty,
        });
    }
    if let Some(t) = unlock_token {
        if !t.trim().is_empty() {
            return Ok(PresenceProof {
                method: PresenceMethod::UnlockToken,
            });
        }
    }
    Err(PresenceError::RequiresHumanPresence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tty_grants_presence() {
        let p = presence_from(true, None).unwrap();
        assert_eq!(p.method, PresenceMethod::Tty);
    }

    #[test]
    fn valid_unlock_token_grants_presence() {
        let p = presence_from(false, Some("s3cret-one-shot")).unwrap();
        assert_eq!(p.method, PresenceMethod::UnlockToken);
    }

    #[test]
    fn agent_caller_is_refused() {
        // no TTY, no token = an agent / non-interactive caller (HP-1)
        assert_eq!(
            presence_from(false, None),
            Err(PresenceError::RequiresHumanPresence)
        );
        // empty token does not count
        assert_eq!(
            presence_from(false, Some("   ")),
            Err(PresenceError::RequiresHumanPresence)
        );
    }
}
