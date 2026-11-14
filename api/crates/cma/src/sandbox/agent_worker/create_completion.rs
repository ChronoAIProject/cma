//! Identity-free completion telemetry for the first Agent Ready publication.
//!
//! The current Agent aggregate has only second-resolution timestamps. Keeping
//! the event timing explicitly unavailable avoids presenting those values as a
//! millisecond admission-to-Ready measurement until a durable millisecond
//! admission boundary is carried through the create path.

use layer2_core::sandbox::{Agent, AgentOrigin};

pub(super) fn emit(agent: &Agent, ready_path: &'static str, runtime_path: &'static str) {
    let create_path = match &agent.origin {
        AgentOrigin::Lifecycle { .. } => "lifecycle",
        AgentOrigin::Manual => "existing_workspace",
        AgentOrigin::Trigger { .. } | AgentOrigin::WorkspaceProfile { .. } => "background",
    };
    let create_class = match &agent.origin {
        AgentOrigin::Trigger { .. } | AgentOrigin::WorkspaceProfile { .. } => "unknown",
        AgentOrigin::Lifecycle { .. } | AgentOrigin::Manual => agent
            .conversation
            .first_message
            .as_ref()
            .map_or(
                "plain",
                |message| {
                    if message.desktop {
                        "desktop"
                    } else {
                        "plain"
                    }
                },
            ),
    };

    // Do not attach this record to the worker span: inherited spans may carry
    // request, user, resource, or token identities into the collector.
    tracing::info!(
        target: "agent_create_completed",
        parent: None,
        event = "agent_create_completed",
        schema_version = 1u8,
        create_path,
        create_class,
        runtime_path,
        ready_path,
        boundary = "ready_commit_ack",
        timing_basis = "unavailable",
        timing_valid = false,
        phase_coverage = "none",
        phase_accounting = "unavailable",
        "Agent create completed"
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn fixed_enums_do_not_include_identity_fields() {
        let fields = [
            "event",
            "schema_version",
            "create_path",
            "create_class",
            "runtime_path",
            "ready_path",
            "boundary",
            "timing_basis",
            "timing_valid",
            "phase_coverage",
            "phase_accounting",
        ];
        assert!(!fields.iter().any(|field| {
            field.contains("id") || field.contains("request") || field.contains("token")
        }));
    }
}
