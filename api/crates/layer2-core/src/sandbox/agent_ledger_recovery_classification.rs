//! Read-only recovery classification from authoritative resource records.
//! Classification never grants admission, changes lifecycle or replays input.

use serde::{Deserialize, Serialize};

use crate::sandbox::{Agent, AgentStatus, AgentWorkStep, Sandbox, SandboxStatus};
use crate::{Caller, Capability};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerRecoveryRouteV1 {
    PreserveUnknown,
    PreserveLive,
    PreserveTerminal,
    OwnerSelectedMigration,
    ParentRecoveryRequired,
    SameAgentQualification,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerRecoveryHistoryV1 {
    NoRecordedConversation,
    RecordedConversation,
    AttemptedOrUncertainInput,
}

/// Counts and enum values only; safe to include in aggregate dry-run evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRecoveryAssessmentV1 {
    pub format: String,
    pub route: LedgerRecoveryRouteV1,
    pub history: LedgerRecoveryHistoryV1,
    pub pending_destructive_steps: u64,
    pub retained_cleanup_obligations: u64,
    pub retained_queued_inputs: u64,
    pub recorded_work_attempts: u32,
    pub native_inspection_required: bool,
    pub original_hold_required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LedgerRecoveryAssessmentRefusal {
    #[error("ledger recovery resource is not available to this caller")]
    NotFound,
    #[error("ledger recovery source lineage or revision is invalid")]
    InvalidSource,
}

/// A record-only GET can describe the required recovery route. Even an empty
/// recorded conversation cannot prove absent native files or provider effects.
/// Inspecting this result must never persist quarantine or a maintenance hold.
pub fn assess_ledger_recovery(
    caller: &Caller,
    sandbox: &Sandbox,
    agent: &Agent,
) -> Result<LedgerRecoveryAssessmentV1, LedgerRecoveryAssessmentRefusal> {
    let owner = caller.subject == sandbox.owner && caller.subject == agent.owner;
    if !(caller.authorization.allows(Capability::ControlAnySandbox)
        || owner && caller.authorization.allows(Capability::UseOwnSandboxes))
    {
        return Err(LedgerRecoveryAssessmentRefusal::NotFound);
    }
    if sandbox.id != agent.sandbox_id
        || sandbox.owner != agent.owner
        || sandbox.revision == 0
        || agent.revision == 0
        || agent.work.next_fence == 0
        || agent.validate_conversation().is_err()
    {
        return Err(LedgerRecoveryAssessmentRefusal::InvalidSource);
    }

    let history = if attempted_input(agent) {
        LedgerRecoveryHistoryV1::AttemptedOrUncertainInput
    } else if agent.conversation.conversation_id.is_some()
        || agent.work.warm_conversation_id.is_some()
    {
        LedgerRecoveryHistoryV1::RecordedConversation
    } else {
        LedgerRecoveryHistoryV1::NoRecordedConversation
    };
    let route = if agent.status.is_live() {
        LedgerRecoveryRouteV1::PreserveLive
    } else if matches!(agent.status, AgentStatus::Deleted | AgentStatus::Stopped) {
        LedgerRecoveryRouteV1::PreserveTerminal
    } else if agent.status != AgentStatus::Failed || !known_ledger_failure(agent) {
        LedgerRecoveryRouteV1::PreserveUnknown
    } else {
        match sandbox.status {
            SandboxStatus::Retiring => LedgerRecoveryRouteV1::OwnerSelectedMigration,
            SandboxStatus::Active => LedgerRecoveryRouteV1::SameAgentQualification,
            SandboxStatus::Paused | SandboxStatus::Failed => {
                LedgerRecoveryRouteV1::ParentRecoveryRequired
            }
            _ => LedgerRecoveryRouteV1::PreserveUnknown,
        }
    };
    Ok(LedgerRecoveryAssessmentV1 {
        format: "ledger-recovery-assessment-v1".into(),
        route,
        history,
        pending_destructive_steps: u64::from(destructive(agent.work.step)),
        retained_cleanup_obligations: agent.work.cleanup_debt.len() as u64,
        retained_queued_inputs: agent.conversation.queued_messages.len() as u64
            + u64::from(agent.conversation.first_message.is_some()),
        recorded_work_attempts: agent.work.attempts,
        native_inspection_required: true,
        original_hold_required: true,
    })
}

fn destructive(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::CloseProcess
            | AgentWorkStep::EnsureProcessClosed
            | AgentWorkStep::RemoveHome
            | AgentWorkStep::RemoveWorktree
            | AgentWorkStep::PublishDeleted
    )
}

fn attempted_input(agent: &Agent) -> bool {
    agent.conversation.active_turn.is_some()
        || agent.conversation.operation.is_some()
        || agent.conversation.first_message.as_ref().is_some_and(|first| {
            first.attempt > 0
                || first.dispatch_generation > 0
                || first.dispatch_fenced_at.is_some()
                || first.write_confirmed_at.is_some()
                || first.execution_observed_at.is_some()
                || first.turn_sequence.is_some()
        })
        // A queued record can retain attempted dispatch. It remains unknown
        // here; a later exact command-lane/provider census may disambiguate it.
        || !agent.conversation.queued_messages.is_empty()
}

fn known_ledger_failure(agent: &Agent) -> bool {
    let Some(failure) = &agent.failure else {
        return false;
    };
    if failure.class == crate::sandbox::store::AGENT_RUNTIME_LEDGER_HOLD {
        return failure.detail == crate::sandbox::store::AGENT_RUNTIME_LEDGER_HOLD_DETAIL;
    }
    // Retained failures predate the typed reachable-runtime hold. Only exact
    // legacy code/detail pairs classify them; arbitrary substring matches do not.
    failure.class == "agent_runtime_rejected"
        && matches!(
            failure.detail.as_str(),
            "mutation_journal_corrupt: runtime mutation journal contains too many entries"
                | "mutation_authority_corrupt: runtime mutation authority contains too many records"
                | "mutation_authority_corrupt: mutation authority contains too many records"
                | "mutation_ledger_capacity: runtime mutation ledger capacity is exhausted"
        )
}

#[cfg(test)]
#[path = "agent_ledger_recovery_classification_tests.rs"]
mod tests;
