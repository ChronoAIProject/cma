use super::*;
use crate::sandbox::{
    model_credit::AgentTurnFunding, task_experience::TaskExperiencePolicy, AgentApprovalDecision,
};

impl AgentCommandService {
    pub fn with_task_experience(self: Arc<Self>, policy: Arc<TaskExperiencePolicy>) -> Arc<Self> {
        Arc::new(Self {
            task_experience: policy,
            ..(*self).clone()
        })
    }

    pub fn with_model_credit_required(self: Arc<Self>, required: bool) -> Arc<Self> {
        Arc::new(Self {
            model_credit_required: required,
            ..(*self).clone()
        })
    }

    pub fn with_model_funding(
        self: Arc<Self>,
        funding: Option<Arc<AgentTurnFunding>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            model_funding: funding,
            ..(*self).clone()
        })
    }

    pub(in crate::sandbox::agent_io) async fn reserve_model_credit(
        &self,
        authorized: &AuthorizedResource,
        request: &CommandReservation,
    ) -> Result<(), CommandAdmissionError> {
        let funding = self
            .model_funding
            .as_ref()
            .ok_or(AgentOperationError::ModelFundingUnsupported)?;
        let lineage = &authorized.snapshot.lineage;
        let agent = lineage.agent.as_ref().ok_or(CommandLedgerError::Corrupt)?;
        self.with_authority_bound(
            authorized,
            command_capability(&request.envelope.command),
            funding.reserve_admission(request, agent, &lineage.sandbox),
        )
        .await?
        .map_err(AgentOperationError::from)?;
        Ok(())
    }

    pub(in crate::sandbox::agent_io) fn require_model_credit(
        &self,
        command: &AgentCommand,
    ) -> Result<(), AgentOperationError> {
        let potentially_billable = matches!(
            command,
            AgentCommand::Message { .. }
                | AgentCommand::Steer { .. }
                | AgentCommand::AnswerApproval {
                    decision: AgentApprovalDecision::Allow | AgentApprovalDecision::Deny,
                    ..
                }
        );
        crate::sandbox::model_credit::require_available(
            self.model_credit_required && potentially_billable,
        )
    }
}
