use super::*;
use layer2_core::sandbox::task_experience::{TaskExperienceCapability, TaskExperiencePolicy};

impl InlineAgentLifecycle {
    pub(crate) fn with_task_experience(
        self: Arc<Self>,
        policy: Arc<TaskExperiencePolicy>,
    ) -> Arc<Self> {
        self.task_experience
            .set(policy)
            .expect("lifecycle policy is configured once before use");
        self
    }

    pub(super) fn require_task_experience(
        &self,
        caller: &Caller,
        request: &AgentLifecycleCreateRequest,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        let allows = |capability| {
            self.task_experience
                .get()
                .is_some_and(|policy| policy.allows(&caller.subject, capability))
        };
        if (request.initial_message.is_some()
            && !allows(TaskExperienceCapability::TaskHomeAdmission))
            || (request.desktop && !allows(TaskExperienceCapability::DesktopEnable))
        {
            return Err(AgentLifecycleProvisioningError::Forbidden);
        }
        Ok(())
    }
}
