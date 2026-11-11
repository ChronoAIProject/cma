//! Nonmutating, authenticated assessment. No hold, lease, or native work is admitted.

use serde::{Deserialize, Serialize};

use super::agent_ledger_recovery_classification::{
    assess_ledger_recovery, LedgerRecoveryHistoryV1, LedgerRecoveryRouteV1,
};
use super::ledger_maintenance::LedgerMaintenancePhaseV1;
use super::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
use super::store::ledger_maintenance::{LedgerMaintenanceReader, LedgerMaintenanceStoreError};
use super::WorkspaceWorkStep;
use crate::{Caller, Capability, SandboxId};

#[derive(Clone, Copy, Debug, Default)]
pub struct LedgerMaintenanceAssessmentPolicy {
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRecoveryCountsV1 {
    pub preserve_unknown: u64,
    pub preserve_live: u64,
    pub preserve_terminal: u64,
    pub owner_selected_migration: u64,
    pub parent_recovery_required: u64,
    pub same_agent_qualification: u64,
    pub no_recorded_conversation: u64,
    pub recorded_conversation: u64,
    pub attempted_or_uncertain_input: u64,
    pub pending_destructive_agent_steps: u64,
    pub retained_cleanup_obligations: u64,
    pub retained_queued_inputs: u64,
    pub recorded_work_attempts: u64,
}

/// Safe projection only. Absence of a marker is not proof of serving readiness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerMaintenanceAssessmentV1 {
    pub format: String,
    pub source_sha256: String,
    pub agents: u64,
    pub workspaces: u64,
    pub worktrees: u64,
    pub recorded_command_lanes: u64,
    pub leased_command_lanes: u64,
    pub pending_parent_steps: u64,
    pub pending_workspace_steps: u64,
    pub pending_workspace_removals: u64,
    pub maintenance_phase: Option<LedgerMaintenancePhaseV1>,
    pub maintenance_epoch: Option<u64>,
    pub accepted_plan_sha256: Option<String>,
    pub source_matches_accepted: Option<bool>,
    pub recovery: LedgerRecoveryCountsV1,
    pub native_effects_unknown: bool,
    pub physical_exclusion_unknown: bool,
    pub backup_verification_required: bool,
    pub recovery_authorized: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LedgerMaintenanceAssessmentError {
    #[error("ledger maintenance assessment is disabled")]
    Disabled,
    #[error("ledger maintenance resource is not available to this caller")]
    NotFound,
    #[error("ledger maintenance assessment authority is inconsistent")]
    InvalidSource,
    #[error("ledger maintenance assessment is unavailable")]
    Unavailable,
}

pub async fn assess_ledger_maintenance(
    reader: &dyn LedgerMaintenanceReader,
    policy: LedgerMaintenanceAssessmentPolicy,
    caller: &Caller,
    sandbox: &SandboxId,
) -> Result<LedgerMaintenanceAssessmentV1, LedgerMaintenanceAssessmentError> {
    use LedgerMaintenanceAssessmentError as Error;
    if !policy.enabled {
        return Err(Error::Disabled);
    }
    if !caller.authorization.allows(Capability::UseOwnSandboxes)
        && !caller.authorization.allows(Capability::ControlAnySandbox)
    {
        return Err(Error::NotFound);
    }
    let inspection = reader
        .inspect(caller, sandbox)
        .await
        .map_err(|error| match error {
            LedgerMaintenanceStoreError::Refused(LedgerMaintenanceAdmissionRefusal::NotFound) => {
                Error::NotFound
            }
            LedgerMaintenanceStoreError::Corrupt => Error::InvalidSource,
            _ => Error::Unavailable,
        })?;
    let source = &inspection.source;
    if &source.sandbox.id != sandbox
        || !(caller.subject == source.sandbox.owner
            || caller.authorization.allows(Capability::ControlAnySandbox))
    {
        return Err(Error::NotFound);
    }
    source
        .validate_inventory(&crate::now_rfc3339())
        .map_err(|_| Error::InvalidSource)?;
    let mut counts = LedgerRecoveryCountsV1::default();
    for agent in &source.agents {
        let assessment = assess_ledger_recovery(caller, &source.sandbox, agent)
            .map_err(|_| Error::InvalidSource)?;
        match assessment.route {
            LedgerRecoveryRouteV1::PreserveUnknown => counts.preserve_unknown += 1,
            LedgerRecoveryRouteV1::PreserveLive => counts.preserve_live += 1,
            LedgerRecoveryRouteV1::PreserveTerminal => counts.preserve_terminal += 1,
            LedgerRecoveryRouteV1::OwnerSelectedMigration => counts.owner_selected_migration += 1,
            LedgerRecoveryRouteV1::ParentRecoveryRequired => counts.parent_recovery_required += 1,
            LedgerRecoveryRouteV1::SameAgentQualification => counts.same_agent_qualification += 1,
        }
        match assessment.history {
            LedgerRecoveryHistoryV1::NoRecordedConversation => counts.no_recorded_conversation += 1,
            LedgerRecoveryHistoryV1::RecordedConversation => counts.recorded_conversation += 1,
            LedgerRecoveryHistoryV1::AttemptedOrUncertainInput => {
                counts.attempted_or_uncertain_input += 1
            }
        }
        counts.pending_destructive_agent_steps += assessment.pending_destructive_steps;
        counts.retained_cleanup_obligations += assessment.retained_cleanup_obligations;
        counts.retained_queued_inputs += assessment.retained_queued_inputs;
        counts.recorded_work_attempts += u64::from(assessment.recorded_work_attempts);
    }
    let (phase, epoch, plan, source_matches) = match &inspection.state {
        Some(state) => {
            let accepted = state.accepted().map_err(|_| Error::InvalidSource)?;
            if accepted.plan().source.sandbox.id != source.sandbox.id
                || accepted.plan().source.sandbox.owner != source.sandbox.owner
            {
                return Err(Error::InvalidSource);
            }
            (
                Some(state.record.phase),
                Some(state.record.binding.maintenance_epoch),
                Some(accepted.sha256().to_owned()),
                Some(&accepted.plan().source == source),
            )
        }
        None => (None, None, None, None),
    };
    Ok(LedgerMaintenanceAssessmentV1 {
        format: "ledger-maintenance-assessment-v1".into(),
        source_sha256: source.source_documents_sha256.clone(),
        agents: source.agents.len() as u64,
        workspaces: source.workspaces.len() as u64,
        worktrees: source.worktrees.len() as u64,
        recorded_command_lanes: source
            .command_lanes
            .iter()
            .filter(|row| row.lane.is_some())
            .count() as u64,
        leased_command_lanes: source
            .command_lanes
            .iter()
            .filter(|row| row.lane.as_ref().is_some_and(|lane| lane.lease.is_some()))
            .count() as u64,
        pending_parent_steps: u64::from(source.sandbox.work.step.is_pending()),
        pending_workspace_steps: source
            .workspaces
            .iter()
            .filter(|row| row.work.step.is_pending())
            .count() as u64,
        pending_workspace_removals: source
            .workspaces
            .iter()
            .filter(|row| row.work.step == WorkspaceWorkStep::Remove)
            .count() as u64,
        maintenance_phase: phase,
        maintenance_epoch: epoch,
        accepted_plan_sha256: plan,
        source_matches_accepted: source_matches,
        recovery: counts,
        native_effects_unknown: true,
        physical_exclusion_unknown: true,
        backup_verification_required: true,
        recovery_authorized: false,
    })
}

#[cfg(test)]
#[path = "ledger_maintenance_assessment_tests.rs"]
mod tests;
