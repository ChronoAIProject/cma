//! Public assessment schema: counts, hashes, phases and explicit unknowns only.

use layer2_core::sandbox::ledger_maintenance_assessment as domain;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct LedgerRecoveryCountsView {
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

impl From<domain::LedgerRecoveryCountsV1> for LedgerRecoveryCountsView {
    fn from(value: domain::LedgerRecoveryCountsV1) -> Self {
        Self {
            preserve_unknown: value.preserve_unknown,
            preserve_live: value.preserve_live,
            preserve_terminal: value.preserve_terminal,
            owner_selected_migration: value.owner_selected_migration,
            parent_recovery_required: value.parent_recovery_required,
            same_agent_qualification: value.same_agent_qualification,
            no_recorded_conversation: value.no_recorded_conversation,
            recorded_conversation: value.recorded_conversation,
            attempted_or_uncertain_input: value.attempted_or_uncertain_input,
            pending_destructive_agent_steps: value.pending_destructive_agent_steps,
            retained_cleanup_obligations: value.retained_cleanup_obligations,
            retained_queued_inputs: value.retained_queued_inputs,
            recorded_work_attempts: value.recorded_work_attempts,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LedgerMaintenanceAssessmentView {
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
    pub maintenance_phase: Option<LedgerMaintenancePhaseView>,
    pub maintenance_epoch: Option<u64>,
    pub accepted_plan_sha256: Option<String>,
    pub source_matches_accepted: Option<bool>,
    pub recovery: LedgerRecoveryCountsView,
    pub native_effects_unknown: bool,
    pub physical_exclusion_unknown: bool,
    pub backup_verification_required: bool,
    pub recovery_authorized: bool,
}

impl From<domain::LedgerMaintenanceAssessmentV1> for LedgerMaintenanceAssessmentView {
    fn from(value: domain::LedgerMaintenanceAssessmentV1) -> Self {
        Self {
            format: value.format,
            source_sha256: value.source_sha256,
            agents: value.agents,
            workspaces: value.workspaces,
            worktrees: value.worktrees,
            recorded_command_lanes: value.recorded_command_lanes,
            leased_command_lanes: value.leased_command_lanes,
            pending_parent_steps: value.pending_parent_steps,
            pending_workspace_steps: value.pending_workspace_steps,
            pending_workspace_removals: value.pending_workspace_removals,
            maintenance_phase: value.maintenance_phase.map(Into::into),
            maintenance_epoch: value.maintenance_epoch,
            accepted_plan_sha256: value.accepted_plan_sha256,
            source_matches_accepted: value.source_matches_accepted,
            recovery: value.recovery.into(),
            native_effects_unknown: value.native_effects_unknown,
            physical_exclusion_unknown: value.physical_exclusion_unknown,
            backup_verification_required: value.backup_verification_required,
            recovery_authorized: value.recovery_authorized,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LedgerMaintenancePhaseView {
    Requested,
    Draining,
    Excluded,
    BackedUp,
    Applying,
    HeldVerified,
    Released,
    RestoreHeld,
    AbortedHeld,
}

impl From<layer2_core::sandbox::ledger_maintenance::LedgerMaintenancePhaseV1>
    for LedgerMaintenancePhaseView
{
    fn from(value: layer2_core::sandbox::ledger_maintenance::LedgerMaintenancePhaseV1) -> Self {
        use layer2_core::sandbox::ledger_maintenance::LedgerMaintenancePhaseV1 as Phase;
        match value {
            Phase::Requested => Self::Requested,
            Phase::Draining => Self::Draining,
            Phase::Excluded => Self::Excluded,
            Phase::BackedUp => Self::BackedUp,
            Phase::Applying => Self::Applying,
            Phase::HeldVerified => Self::HeldVerified,
            Phase::Released => Self::Released,
            Phase::RestoreHeld => Self::RestoreHeld,
            Phase::AbortedHeld => Self::AbortedHeld,
        }
    }
}
