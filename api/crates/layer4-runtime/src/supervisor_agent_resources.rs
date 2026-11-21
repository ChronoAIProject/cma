//! Ordered preparation under the supervisor's existing mutation critical section.
use layer3_adapter::wire::{
    AgentResourcesPlan, AgentResourcesReceipt, FencedReceipt, FencedRequest, ResponseFrame,
};

use super::{agent_validation, Supervisor};
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::Operation;

impl Supervisor {
    pub(super) async fn authorize_agent_resources(
        &self,
        request: &FencedRequest<AgentResourcesPlan>,
    ) -> Result<()> {
        // This path is for an owned runtime. Unowned preparation retains its
        // existing individual requests and installation/credential gates.
        self.require_credentialed().await?;
        crate::worktree::validate_add_request(&request.request.worktree)?;
        agent_validation::materialize_home(&request.request.home)?;
        // Both original journals and per-resource fences remain authoritative.
        // A partial admission or effect is recoverable only with the same two
        // persisted intents; it does not authorize a fallback or blind replay.
        self.mutations
            .admit(
                request.authority_fence,
                &[
                    Operation::add_worktree(&request.request.worktree),
                    Operation::create_agent_home(&request.request.home),
                ],
            )
            .await
    }

    pub(super) async fn prepare_agent_resources(
        &self,
        fence: u64,
        plan: AgentResourcesPlan,
    ) -> Result<ResponseFrame> {
        // dispatch holds the operation gate across both effects, with the
        // original persistent-home, checkpoint and writer authority checks.
        let ResponseFrame::AddWorktree(worktree) = self.add_worktree(fence, plan.worktree).await?
        else {
            return Err(invalid_receipt());
        };
        let ResponseFrame::MaterializeAgentHome(home) =
            self.materialize_agent_home(fence, plan.home).await?
        else {
            return Err(invalid_receipt());
        };
        if worktree.authority_fence != fence || home.authority_fence != fence {
            return Err(invalid_receipt());
        }
        Ok(ResponseFrame::PrepareAgentResources(FencedReceipt::new(
            fence,
            AgentResourcesReceipt {
                worktree: worktree.receipt,
                home: home.receipt,
            },
        )))
    }
}

fn invalid_receipt() -> RuntimeError {
    RuntimeError::rejected(
        "agent_resources_receipt",
        "Agent resource receipt does not match its operation",
    )
}
