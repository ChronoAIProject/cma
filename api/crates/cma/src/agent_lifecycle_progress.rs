//! Report observed provisioning gates without waiting for the whole Agent.
use super::*;
use layer2_core::sandbox::{
    store::AgentLogStore, Agent, AgentLogEntry, AgentLogKind, AgentStatus, AgentWorkStep,
};

#[path = "agent_lifecycle_readiness.rs"]
mod readiness;

impl InlineAgentLifecycle {
    pub(super) async fn progress(
        &self,
        lease: &LifecycleLease,
        phase: &str,
        status: &str,
        message: &str,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        self.operations
            .record_progress(lease, phase, status, message)
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)
    }

    pub(super) async fn wait_for_agent_workspace(
        &self,
        caller: &Caller,
        sandbox_id: &SandboxId,
        lease: &LifecycleLease,
        expected_workspace: Option<&layer2_core::WorkspaceId>,
    ) -> Result<layer2_core::WorkspaceId, AgentLifecycleProvisioningError> {
        tokio::time::timeout(Duration::from_secs(180), async {
            let started = tokio::time::Instant::now();
            let mut sandbox_ready = false;
            let mut key_ready = false;
            let fallback = tokio::sync::Notify::new();
            let readiness = self
                .native_prepared
                .as_ref()
                .map(|store| store.agent_parent_ready.as_ref())
                .unwrap_or(&fallback);
            loop {
                // Prepared Agents are admitted only after this exact parent wait.
                // Register before authenticated reads; the hint supplies no state.
                let changed = readiness.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let observation = readiness::observe(
                    &self.sandbox,
                    &self.workspaces,
                    caller,
                    sandbox_id,
                    expected_workspace,
                )
                .await?;
                let sandbox = &observation.sandbox;
                if !sandbox_ready
                    && (sandbox.runtime.is_some() || sandbox.status == SandboxStatus::Active)
                {
                    self.progress_pair(
                        lease,
                        ("sandbox_ready", "ready", "Agent Sandbox is ready"),
                        (
                            "key_scope_ready",
                            "running",
                            "Configuring Agent key permissions",
                        ),
                    )
                    .await?;
                    sandbox_ready = true;
                }
                // Active is also the established readiness gate for retained
                // Sandboxes. Fresh managed Sandboxes publish delivered first.
                if !key_ready
                    && (sandbox.credential.delivered || sandbox.status == SandboxStatus::Active)
                {
                    self.progress_pair(
                        lease,
                        (
                            "key_scope_ready",
                            "ready",
                            "Agent key permissions are ready",
                        ),
                        ("workspace_runtime", "running", "Preparing the workspace"),
                    )
                    .await?;
                    key_ready = true;
                }
                if let Some(id) = observation
                    .ready_workspace(&self.workspaces, caller)
                    .await?
                {
                    return Ok(id);
                }
                tokio::select! {
                    _ = &mut changed => {},
                    _ = tokio::time::sleep(worker::readiness_delay(started)) => {},
                }
            }
        })
        .await
        .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?
    }

    pub(super) async fn progress_pair(
        &self,
        lease: &LifecycleLease,
        first: (&str, &str, &str),
        second: (&str, &str, &str),
    ) -> Result<(), AgentLifecycleProvisioningError> {
        self.operations
            .record_progress_many(lease, &[first, second])
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)
    }
}

#[async_trait]
impl AgentLifecycleOperationReader for InlineAgentLifecycle {
    async fn cancel_unallocated(
        &self,
        caller: &Caller,
        operation_id: &str,
    ) -> Result<AgentLifecycleOperationView, AgentLifecycleOperationReadError> {
        if !may_cancel_creation(caller) {
            return Err(AgentLifecycleOperationReadError::Forbidden);
        }
        let record = self
            .operations
            .read_for_owner(&caller.subject, operation_id)
            .await
            .map_err(|_| AgentLifecycleOperationReadError::Unavailable)?
            .ok_or(AgentLifecycleOperationReadError::NotFound)?;
        if record.creation_fence.is_none() {
            return Err(AgentLifecycleOperationReadError::Conflict("This creation predates durable creation evidence. Recover the original operation and confirm its allocation before releasing the reservation.".into()));
        }
        if !self.operations.creation_terminal_settlement_enabled() {
            return Err(AgentLifecycleOperationReadError::Unavailable);
        }
        if !self
            .operations
            .cancel_fenced_creation(&caller.subject, &record)
            .await
            .map_err(|_| AgentLifecycleOperationReadError::Unavailable)?
        {
            return Err(AgentLifecycleOperationReadError::Conflict("Creation can no longer be cancelled before allocation. Read the exact creation outcome and settle any associated Sandbox through its lifecycle.".into()));
        }
        let settled = self
            .operations
            .read_for_owner(&caller.subject, operation_id)
            .await
            .map_err(|_| AgentLifecycleOperationReadError::Unavailable)?
            .ok_or(AgentLifecycleOperationReadError::Unavailable)?;
        if settled.status != "failed" || settled.sandbox_id.is_some() || settled.agent_id.is_some()
        {
            return Err(AgentLifecycleOperationReadError::Unavailable);
        }
        Ok(settled.view())
    }

    async fn read(
        &self,
        caller: &Caller,
        operation_id: &str,
    ) -> Result<AgentLifecycleOperationView, AgentLifecycleOperationReadError> {
        let record = self
            .operations
            .read_for_owner(&caller.subject, operation_id)
            .await
            .map_err(|_| AgentLifecycleOperationReadError::Unavailable)?
            .ok_or(AgentLifecycleOperationReadError::NotFound)?;
        let mut view = record.view();
        if may_cancel_creation(caller)
            && self.operations.creation_terminal_settlement_enabled()
            && matches!(
                record.status.as_str(),
                "queued" | "running" | "waiting_for_connection"
            )
            && record.sandbox_id.is_none()
            && record.agent_id.is_none()
            && record
                .creation_fence
                .as_ref()
                .is_some_and(|fence| self.operations.supports_creation(fence))
        {
            view.can_cancel = Some(true);
        }
        view.callback_url = callback_url(
            self.agent_records.as_ref(),
            &self.agent_page_origin,
            &record.id,
            record.agent_id.as_deref(),
        )
        .await;
        if record.status == "failed"
            && record.error.as_deref().is_some_and(|error| {
                error
                    == AgentLifecycleProvisioningError::Conflict(worker::PREPARATION_FAILED.into())
                        .to_string()
            })
        {
            view.error = self
                .preparation_failure_for(
                    caller,
                    record.agent_id.as_deref(),
                    record.updated_at.timestamp_millis(),
                )
                .await
                .map(|detail| AgentLifecycleProvisioningError::Conflict(detail).to_string())
                .or(view.error);
        }
        Ok(view)
    }
}

fn may_cancel_creation(caller: &Caller) -> bool {
    caller.authorization.allows(Capability::UseOwnSandboxes)
        && caller.authorization.allows(Capability::CreateSandbox)
}

impl InlineAgentLifecycle {
    async fn preparation_failure_for(
        &self,
        caller: &Caller,
        agent_id: Option<&str>,
        operation_updated_ms: i64,
    ) -> Option<String> {
        let id = AgentId(agent_id?.into());
        let store = self.native_prepared.as_ref()?;
        tokio::time::timeout(Duration::from_secs(2), async {
            let agent = self.agents.read(caller, &id).await.ok()?;
            let entry = store
                .agent_log_entry(&agent.id, agent.conversation.last_seq)
                .await
                .ok()??;
            preparation_diagnostic(&agent, &entry, operation_updated_ms)
        })
        .await
        .ok()
        .flatten()
    }
}

fn preparation_diagnostic(
    agent: &Agent,
    entry: &AgentLogEntry,
    operation_updated_ms: i64,
) -> Option<String> {
    let AgentLogKind::WorkStepFailed { step, failure } = &entry.kind else {
        return None;
    };
    let occurred = layer2_core::seconds_from_rfc3339(&entry.appended_at)?;
    if agent.status != AgentStatus::Failed
        || agent.ready_at.is_some()
        || agent.failure.as_ref() != Some(failure)
        || agent.id != entry.agent_id
        || agent.conversation.last_seq != entry.seq
        || entry.seq == 0
        || agent.updated_at != entry.appended_at
        || occurred > u64::try_from(operation_updated_ms).ok()? / 1000
    {
        return None;
    }
    let action = match step {
        AgentWorkStep::WaitForWorkspace => "waiting for the Workspace",
        AgentWorkStep::AddWorktree => "adding the Worktree",
        AgentWorkStep::MaterializeHome => "preparing the Agent home",
        AgentWorkStep::InstallSkill => "installing an Agent skill",
        AgentWorkStep::OpenConversation => "opening the Agent conversation",
        AgentWorkStep::PublishReady => "publishing Agent readiness",
        _ => return None,
    };
    let category = match failure.class.as_str() {
        "agent_runtime_rejected" => "agent_runtime_rejected",
        "agent_runtime_unsupported" => "agent_runtime_unsupported",
        "agent_runtime_unavailable" => "agent_runtime_unavailable",
        "agent_runtime_contract_mismatch" => "agent_runtime_contract_mismatch",
        _ => "agent_preparation_failed",
    };
    Some(format!(
        "{category}: Agent preparation failed while {action}."
    ))
}

#[cfg(test)]
#[path = "agent_lifecycle_preparation_diagnostic_tests.rs"]
mod preparation_diagnostic_tests;

/// Missing or Deleted Agents yield null; observation errors leave the link unknown.
pub(super) async fn callback_url(
    store: &dyn layer2_core::sandbox::store::AgentStore,
    origin: &layer1_public::agent_page::AgentPageOrigin,
    operation_id: &str,
    agent_id: Option<&str>,
) -> Option<Option<String>> {
    let id = agent_id?;
    match store.get_agent(&AgentId(id.into())).await {
        Ok(agent) => Some(
            agent
                .filter(|agent| agent.status != layer2_core::sandbox::AgentStatus::Deleted)
                .and_then(|agent| origin.agent_url(agent.id.as_str())),
        ),
        Err(_) => {
            tracing::warn!(
                operation_id,
                agent_id = id,
                "could not observe lifecycle Agent page availability"
            );
            None
        }
    }
}
