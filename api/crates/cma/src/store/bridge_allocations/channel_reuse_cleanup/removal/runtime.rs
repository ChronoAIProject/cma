use super::*;
use crate::sandbox::LiveAgentRegistry;
use layer3_adapter::{
    AgentCloseRequest, AgentHomeRemoveRequest, AgentSlot, SandboxAdapter, WorktreeRef,
};

impl MongoBridgeAllocationStore {
    /// Every invocation obtains current authority and quiescence under the
    /// retained exclusion. Serialized progress alone cannot invoke runtime IO.
    pub(crate) async fn run_channel_reuse_child_removal(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        request: &ChannelReuseCleanupRequest,
        adapter: &dyn SandboxAdapter,
        registry: &LiveAgentRegistry,
        deployment_id: &str,
    ) -> Result<ChannelReuseRemovalState, AllocationError> {
        let state = self
            .prepare_channel_reuse_child_removal(permit, request)
            .await?;
        if state.phase == ChannelReuseRemovalPhase::Complete {
            return Ok(state);
        }
        let sandbox = &permit.parent().lineage.sandbox;
        if sandbox.runtime.as_ref() != Some(&state.runtime) {
            return Err(AllocationError::IdentityMismatch);
        }
        let runtime = crate::sandbox::runtime_attachment::adopt_bound_runtime(
            adapter,
            sandbox,
            deployment_id,
        )
        .await
        .map_err(|_| AllocationError::Unavailable)?;
        let fresh = self
            .prepare_channel_reuse_child_removal(permit, request)
            .await?;
        if fresh != state {
            return Err(AllocationError::Conflict);
        }
        permit.require_current()?;
        let slot = AgentSlot::new(state.seal.target.agent_id.to_string());
        let result = match state.phase {
            ChannelReuseRemovalPhase::Intent => {
                registry.fence(&state.seal.target.agent_id, None, state.authority_fence);
                adapter
                    .close_agent(
                        &runtime,
                        state.authority_fence,
                        &AgentCloseRequest::new(format!("agent:{}:close:v1", slot.as_str()), slot),
                    )
                    .await
            }
            ChannelReuseRemovalPhase::ProcessClosed => {
                adapter
                    .remove_agent_home(
                        &runtime,
                        state.authority_fence,
                        &AgentHomeRemoveRequest::new(
                            format!("agent:{}:home:remove:v1", slot.as_str()),
                            slot,
                        ),
                    )
                    .await
            }
            ChannelReuseRemovalPhase::HomeRemoved => {
                adapter
                    .remove_worktree(
                        &runtime,
                        state.authority_fence,
                        &WorktreeRef::new(
                            format!(
                                "worktree:{}:{}:remove:v1",
                                state.workspace_name(),
                                state.worktree_name()
                            ),
                            state.workspace_name(),
                            state.worktree_name(),
                        ),
                    )
                    .await
            }
            ChannelReuseRemovalPhase::WorktreeRemoved => Ok(()),
            ChannelReuseRemovalPhase::Complete => unreachable!(),
        };
        // Lost responses retain the exact same operation and exclusion. No age
        // or timeout can publish completion or release a reservation.
        result.map_err(|_| AllocationError::Unavailable)?;
        permit.require_current()?;
        self.change_removal(permit, request, Some(&state)).await
    }
}
