use super::*;
use layer2_core::sandbox::task_experience::{TaskExperienceCapability, TaskExperiencePolicy};

impl InlineAgentLifecycle {
    /// Share the process's released-default authority for machine images.
    pub(crate) fn with_released_runtime(
        self: Arc<Self>,
        released: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
    ) -> Arc<Self> {
        self.released
            .set(released)
            .expect("released runtime is configured once before use");
        self
    }

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
        if ((request.initial_message.is_some() || request.title_mode.is_some())
            && !allows(TaskExperienceCapability::TaskHomeAdmission))
            || ((request.desktop || request.machine_included)
                && !allows(TaskExperienceCapability::DesktopEnable))
        {
            return Err(AgentLifecycleProvisioningError::Forbidden);
        }
        Ok(())
    }
}
