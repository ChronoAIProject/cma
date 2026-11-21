use super::*;
use crate::sandbox::{task_experience::TaskExperiencePolicy, AgentApprovalDecision};

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
