//! Rendering an owner's decision into what this bridge understands.
//!
//! Split from the client so both stay within the project's file-size limit, on
//! a division that is real: that file is a conversation, and this is the one
//! translation in it where a wrong answer grants more than somebody asked for.

use crate::{PermissionDecision, PermissionOptionKind, RuntimePermission};

/// This bridge's name for how much the agent may do unattended.
///
/// The bridge exposes modes rather than the approval policy the kernel's own
/// configuration file uses, and it starts every session in `agent` regardless
/// of that file — which is why this is sent explicitly and why failing to send
/// it fails the session.
pub(super) fn mode_for(permission: RuntimePermission) -> &'static str {
    match permission {
        RuntimePermission::Guarded => "read-only",
        RuntimePermission::Autonomous => "agent",
        // No approvals and no confinement inside the sandbox. Survivable only
        // because the sandbox is still the boundary.
        RuntimePermission::Unrestricted => "agent-full-access",
    }
}

/// Render an owner's intent into one of the options this bridge offered.
///
/// The options are the bridge's own and differ per request, so the intent is
/// matched against what was actually on offer rather than against a fixed set.
/// An intent with nothing to match is cancelled, which every ACP agent
/// understands and which stops the turn rather than letting it proceed — the
/// safe direction when the answer was "no" and we cannot say which kind.
pub(super) fn outcome_for(
    decision: PermissionDecision,
    options: &[crate::PermissionOption],
) -> serde_json::Value {
    let wanted: &[PermissionOptionKind] = match decision {
        PermissionDecision::Allow => &[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ],
        PermissionDecision::Deny => &[
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ],
        PermissionDecision::Abort => &[],
    };
    for kind in wanted {
        if let Some(option) = options.iter().find(|o| &o.kind == kind) {
            return serde_json::json!({
                "outcome": "selected",
                "optionId": option.option_id.0.to_string(),
            });
        }
    }
    serde_json::json!({ "outcome": "cancelled" })
}

#[cfg(test)]
#[path = "acp_tests.rs"]
mod permission_tests;
