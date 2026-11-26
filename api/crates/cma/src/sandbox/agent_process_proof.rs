//! Read-only proof of the retained process before owner Stop settlement or restart.
use layer2_core::sandbox::{
    store::AgentRestartSource, Agent, AgentWorkReceipt, AgentWorkStep, Sandbox, SandboxStatus,
    Workspace, WorkspaceStatus, Worktree,
};
use layer2_core::seconds_from_rfc3339;
use layer3_adapter::wire::AgentProcessCloseProof;
use layer3_adapter::{AdapterError, RuntimeHandle, SandboxAdapter};

pub(super) enum CloseProofPurpose {
    SettleStop,
    AdmitRestart,
}

pub(super) enum CloseProofError {
    Unproved,
    Adapter(AdapterError),
}

impl From<AdapterError> for CloseProofError {
    fn from(error: AdapterError) -> Self {
        Self::Adapter(error)
    }
}

pub(super) async fn prove_closed(
    adapter: &dyn SandboxAdapter,
    agent: &Agent,
    sandbox: &Sandbox,
    workspace: &Workspace,
    worktree: &Worktree,
    runtime: &RuntimeHandle,
    purpose: CloseProofPurpose,
) -> Result<AgentRestartSource, CloseProofError> {
    if sandbox.id != agent.sandbox_id
        || sandbox.owner != agent.owner
        || sandbox.status != SandboxStatus::Active
        || sandbox.lifecycle.operation.is_some()
        || workspace.status != WorkspaceStatus::Ready
        || workspace.id != agent.workspace_id
        || workspace.sandbox_id != sandbox.id
        || workspace.owner != agent.owner
        || agent.validate_prerequisites(worktree).is_err()
        || sandbox
            .runtime
            .as_ref()
            .is_none_or(|binding| binding.runtime_id != runtime.0)
    {
        return Err(AdapterError::Rejected(
            "Agent lineage does not permit Close verification".into(),
        )
        .into());
    }
    let conversation_id = agent
        .conversation
        .conversation_id
        .clone()
        .ok_or(CloseProofError::Unproved)?;
    let (open, closed, request) = matching_receipts(agent).ok_or(CloseProofError::Unproved)?;
    let native = adapter.runtime_recovery().ok_or_else(|| {
        AdapterError::Unsupported("native Agent Close proof is unavailable".into())
    })?;
    let status = adapter.runtime_status(runtime).await?;
    if sandbox
        .runtime
        .as_ref()
        .and_then(|binding| binding.boot_id.as_ref())
        != Some(&status.boot_id)
        || sandbox.runtime_process_matches(&status) != Ok(true)
    {
        return Err(AdapterError::Rejected(
            "Agent runtime changed before Close verification".into(),
        )
        .into());
    }
    let identity = match purpose {
        CloseProofPurpose::SettleStop => status.ready_process_identity()?,
        CloseProofPurpose::AdmitRestart => status.restart_runtime()?,
    };
    let proof = AgentProcessCloseProof {
        runtime: identity,
        close: request,
        open_request_sha256: open.request_sha256.clone(),
        worktree_path: worktree.path.clone(),
    };
    if !native.prove_agent_process_closed(runtime, &proof).await? {
        return Err(CloseProofError::Unproved);
    }
    Ok(AgentRestartSource {
        conversation_id,
        predecessor_open: open.clone(),
        predecessor_close: closed.clone(),
        runtime: proof.runtime,
        worktree_revision: worktree.revision,
        workspace_revision: workspace.revision,
        sandbox_revision: sandbox.revision,
    })
}

fn matching_receipts(
    agent: &Agent,
) -> Option<(
    &AgentWorkReceipt,
    &AgentWorkReceipt,
    layer3_adapter::AgentCloseRequest,
)> {
    if !agent.work.cleanup_debt.is_empty() {
        return None;
    }
    let open = agent
        .work
        .receipts
        .iter()
        .find(|receipt| receipt.step == AgentWorkStep::OpenConversation)?;
    let close_request = agent.close_process_request().ok()?;
    let mut closes = agent
        .work
        .receipts
        .iter()
        .filter(|receipt| receipt.step == AgentWorkStep::CloseProcess);
    let closed = closes.next()?;
    if closes.next().is_some()
        || closed.operation_id != close_request.operation_id
        || closed.request_sha256 != close_request.request_sha256
        || seconds_from_rfc3339(&closed.completed_at)? < seconds_from_rfc3339(&open.completed_at)?
    {
        return None;
    }
    Some((open, closed, close_request))
}

pub(crate) struct RestartObservation {
    pub adapter: std::sync::Arc<dyn SandboxAdapter>,
    pub deployment_id: String,
}

#[async_trait::async_trait]
impl layer2_core::sandbox::AgentRestartObservation for RestartObservation {
    async fn prove_closed(
        &self,
        agent: &Agent,
        sandbox: &Sandbox,
        workspace: &Workspace,
        worktree: &Worktree,
    ) -> Result<AgentRestartSource, layer2_core::sandbox::AgentServiceError> {
        use layer2_core::sandbox::AgentServiceError;
        let runtime = crate::sandbox::runtime_attachment::adopt_bound_runtime(
            self.adapter.as_ref(),
            sandbox,
            &self.deployment_id,
        )
        .await
        .map_err(|error| AgentServiceError::Unavailable(error.to_string()))?;
        prove_closed(
            self.adapter.as_ref(),
            agent,
            sandbox,
            workspace,
            worktree,
            &runtime,
            CloseProofPurpose::AdmitRestart,
        )
        .await
        .map_err(|error| match error {
            CloseProofError::Unproved if agent.conversation.active_turn.is_some() => {
                AgentServiceError::RestartTurnUnsettled
            }
            CloseProofError::Unproved | CloseProofError::Adapter(AdapterError::Rejected(_)) => {
                AgentServiceError::ControlConflict
            }
            CloseProofError::Adapter(AdapterError::Unsupported(_)) => {
                AgentServiceError::RestartRuntimeUnsupported
            }
            CloseProofError::Adapter(other) => AgentServiceError::Unavailable(other.to_string()),
        })
    }
}
