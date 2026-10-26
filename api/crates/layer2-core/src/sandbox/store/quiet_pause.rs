//! Owner-consented quiescence is independent of automatic inactivity policy.

use std::collections::BTreeSet;

use crate::sandbox::{AgentStatus, Sandbox, SandboxAgentActivity, SandboxEventKind, SandboxStatus};
use crate::{seconds_from_rfc3339, SubjectId};

use super::{SandboxMaintenanceApplication, SandboxMaintenanceOutcome, SandboxStoreError};

/// Decide an explicitly approved pause from a complete, transactionally fenced
/// Agent activity projection. The store must atomically bind this result to its
/// still-approved workflow record and fence every live Agent before persisting.
/// An owner approval does not authorize interrupting an active or queued turn.
#[doc(hidden)]
pub fn apply_consented_quiet_pause(
    current: &Sandbox,
    live_agents: &[SandboxAgentActivity],
    owner: &SubjectId,
    requested_at: &str,
) -> Result<SandboxMaintenanceApplication, SandboxStoreError> {
    let now = seconds_from_rfc3339(requested_at).ok_or(SandboxStoreError::InvalidTransaction(
        "consented pause time is invalid",
    ))?;
    if &current.owner != owner {
        return Err(SandboxStoreError::InvalidTransaction(
            "consented pause owner differs",
        ));
    }
    current
        .validate()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let ineligible =
        || SandboxMaintenanceApplication::Complete(SandboxMaintenanceOutcome::NotEligible);
    if current.status != SandboxStatus::Active || live_agents.is_empty() {
        return Ok(ineligible());
    }
    let mut seen = BTreeSet::new();
    if live_agents.iter().any(|activity| {
        activity.sandbox_id != current.id
            || !seen.insert(&activity.agent_id)
            || !matches!(activity.status, AgentStatus::Ready | AgentStatus::Paused)
            || activity.active_turn.is_some()
            || activity.work_pending
            || activity.stop_requested
            || activity.queued_messages_present
            || activity
                .conversation_id
                .as_ref()
                .is_none_or(|id| id.is_empty() || id.len() > 512)
            || activity
                .runtime_generation
                .is_none_or(|generation| generation == 0)
            || activity.last_seq == 0
            || activity.newest_log.as_ref().is_none_or(|log| {
                log.agent_id != activity.agent_id
                    || log.seq != activity.last_seq
                    || seconds_from_rfc3339(&log.appended_at).is_none_or(|at| at > now)
            })
    }) {
        return Ok(ineligible());
    }
    let mut sandbox = current.clone();
    if sandbox
        .begin_pause(
            requested_at.to_owned(),
            Some("Applying your approved service access update.".into()),
        )
        .is_err()
    {
        return Ok(ineligible());
    }
    sandbox
        .validate()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    Ok(SandboxMaintenanceApplication::Persist {
        expected_revision: current.revision,
        sandbox: Box::new(sandbox),
        event: SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Pausing,
        },
    })
}

#[cfg(test)]
#[path = "quiet_pause_tests.rs"]
mod tests;
