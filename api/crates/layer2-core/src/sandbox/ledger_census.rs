//! Recovery planning observations, never authority to retry, retire or erase.
use serde::{Deserialize, Serialize};

use super::{
    Agent, AgentStatus, AgentWorkStep, Sandbox, SandboxModelError, SandboxStatus, Workspace,
    Worktree,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerFailureCause {
    JournalLifetimeBound,
    AuthorityLifetimeBound,
    SharedLedgerCapacity,
    OtherOrUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerCensusAssessment {
    pub cause: LedgerFailureCause,
    pub agent_state: AgentStatus,
    pub parent_state: SandboxStatus,
    pub agent_revision: u64,
    pub parent_revision: u64,
    pub scheduled_step: AgentWorkStep,
    pub cleanup_scheduled: bool,
    pub cleanup_debt: usize,
    pub lease_present: bool,
    pub conversation_present: bool,
    /// Includes retained terminal records; presence does not mean pending input.
    pub input_state_present: bool,
    pub attempted_or_recorded_work: bool,
    pub retained_home_present: bool,
    pub runtime_generation: Option<u64>,
    pub recovery_route: String,
    pub preserve: Vec<String>,
    pub filesystem_evidence: String,
    pub provider_effects: String,
    pub recovery_authorized: bool,
}

impl Agent {
    pub fn ledger_census(
        &self,
        parent: &Sandbox,
        workspace: &Workspace,
        worktree: &Worktree,
    ) -> Result<LedgerCensusAssessment, SandboxModelError> {
        parent.validate()?;
        self.validate_conversation()?;
        self.validate_worktree_recoveries()?;
        if self.sandbox_id != parent.id
            || self.owner != parent.owner
            || workspace.sandbox_id != parent.id
            || workspace.owner != parent.owner
            || worktree.sandbox_id != parent.id
            || worktree.owner != parent.owner
            || self.workspace_id != workspace.id
            || worktree.workspace_id != workspace.id
            || self.worktree_id != worktree.id
            || self.revision == 0
            || workspace.revision == 0
            || worktree.revision == 0
        {
            return Err(SandboxModelError::invalid_value("ledger_census.lineage"));
        }
        let cause = self
            .failure
            .as_ref()
            .map_or(LedgerFailureCause::OtherOrUnknown, |failure| {
                // Diagnostic classification only. A matching string never grants recovery.
                if failure
                    .detail
                    .contains("runtime mutation journal contains too many")
                {
                    LedgerFailureCause::JournalLifetimeBound
                } else if failure
                    .detail
                    .contains("runtime mutation authority contains too many records")
                {
                    LedgerFailureCause::AuthorityLifetimeBound
                } else if failure
                    .detail
                    .contains("runtime mutation ledger capacity is exhausted")
                {
                    LedgerFailureCause::SharedLedgerCapacity
                } else {
                    LedgerFailureCause::OtherOrUnknown
                }
            });
        let conversation_present =
            self.conversation.conversation_id.is_some() || self.work.warm_conversation_id.is_some();
        let input_state_present = self.conversation.active_turn.is_some()
            || !self.conversation.queued_messages.is_empty()
            || self.conversation.first_message.is_some()
            || self.conversation.operation.is_some();
        let cleanup_scheduled = matches!(
            self.work.step,
            AgentWorkStep::EnsureProcessClosed
                | AgentWorkStep::RemoveHome
                | AgentWorkStep::RemoveWorktree
                | AgentWorkStep::PublishDeleted
        );
        let route = if self.status != AgentStatus::Failed {
            "changed_agent_state_requires_reclassification"
        } else if matches!(
            parent.status,
            SandboxStatus::Retiring | SandboxStatus::Retired
        ) {
            "owner_disposition_then_supported_workload_migration"
        } else if parent.status == SandboxStatus::Active {
            "qualified_recovery_assessment_required"
        } else {
            "parent_recovery_required_before_agent_assessment"
        };
        let mut preserve = vec![
            "complete_home_backup",
            "worktree_files_and_git_state",
            "both_mutation_stores_and_archives",
            "agent_history_and_operation_keys",
            "configuration_and_artifact_references",
        ];
        if conversation_present || input_state_present {
            preserve.push("native_conversation_and_input_state_no_automatic_replay");
        }
        if cleanup_scheduled || !self.work.cleanup_debt.is_empty() {
            preserve.push("pending_cleanup_intent_and_receipts_hold_before_restart");
        }
        Ok(LedgerCensusAssessment {
            cause,
            agent_state: self.status,
            parent_state: parent.status,
            agent_revision: self.revision,
            parent_revision: parent.revision,
            scheduled_step: self.work.step,
            cleanup_scheduled,
            cleanup_debt: self.work.cleanup_debt.len(),
            lease_present: self.work.lease.is_some(),
            conversation_present,
            input_state_present,
            attempted_or_recorded_work: self.work.attempts > 0
                || !self.work.receipts.is_empty()
                || !self.work.worktree_recoveries.is_empty()
                || self.conversation.last_seq > 0,
            retained_home_present: parent.lifecycle.home.is_some(),
            runtime_generation: parent
                .retained_runtime_binding()
                .and_then(|r| r.provision_generation),
            recovery_route: route.into(),
            preserve: preserve.into_iter().map(String::from).collect(),
            filesystem_evidence: "not_observed".into(),
            provider_effects: "absence_not_proven".into(),
            recovery_authorized: false,
        })
    }
}
