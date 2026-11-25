use super::*;
use layer2_core::agent_ownership::{
    execution_authority, Authority, OwnershipCoordinator, OwnershipToken,
};

impl LiveAgentRegistry {
    pub(crate) fn owned_target_is_recording(
        &self,
        target: &LiveAgentTarget,
        token: &OwnershipToken,
    ) -> bool {
        self.lock()
            .entries
            .get(&target.agent_id)
            .is_some_and(|entry| {
                entry.published
                    && !entry.task.is_finished()
                    && entry.conversation_id == target.conversation_id
                    && entry.runtime_generation == target.runtime_generation
                    && entry
                        .manager_authority
                        .as_ref()
                        .is_some_and(|owner| owner.token == *token && owner.is_current())
            })
    }

    pub(crate) fn install_ownership(&self, coordinator: Arc<OwnershipCoordinator>) {
        let _ = self.ownership.set(coordinator);
    }

    pub(super) fn attachment_owner(
        &self,
        id: &AgentId,
    ) -> Result<Option<Arc<Authority>>, LiveAgentRegistryError> {
        let Some(coordinator) = self.ownership.get() else {
            return Ok(None);
        };
        execution_authority()
            .filter(|owner| {
                owner.token.agent_id == *id
                    && owner.token.instance_id == coordinator.instance_id
                    && owner.is_current()
                    && coordinator.is_ready()
            })
            .map(Some)
            .ok_or(LiveAgentRegistryError::StaleGeneration)
    }

    /// Local cancellation does not publish a durable disconnect for a successor.
    pub(crate) fn detach_owner(&self, token: &OwnershipToken) {
        let mut state = self.lock();
        for staged in [false, true] {
            let entries = if staged {
                &mut state.staged
            } else {
                &mut state.entries
            };
            if entries.get(&token.agent_id).is_some_and(|entry| {
                entry
                    .manager_authority
                    .as_ref()
                    .is_some_and(|owner| owner.token == *token)
            }) {
                if let Some(entry) = entries.remove(&token.agent_id) {
                    entry.task.abort();
                }
            }
        }
    }
}
