use super::service::unavailable;
use super::*;
use crate::now_rfc3339;
use crate::sandbox::{
    AgentServiceError, AgentStatus, SandboxServiceError, SandboxStatus, WorkspaceServiceError,
    WorkspaceStatus,
};

impl BindingService {
    /// Recovery reads bounded pages; every side effect is guarded by durable resource
    /// admission or lifecycle CAS, so multiple controllers may run this safely.
    pub async fn reconcile(&self, limit: u16) -> Result<(), BindingError> {
        let mut after = None;
        loop {
            let rows = self.store.pending_bindings(after.as_deref(), limit).await?;
            if rows.is_empty() {
                break;
            }
            for binding in &rows {
                if let Err(error) = self.reconcile_binding(binding).await {
                    tracing::warn!(binding_id = %binding.binding_id, %error, "binding recovery will retry");
                }
            }
            after = rows.last().map(|binding| binding.binding_id.clone());
        }
        let mut after = None;
        loop {
            let rows = self
                .store
                .pending_binding_conversations(after.as_deref(), limit)
                .await?;
            if rows.is_empty() {
                break;
            }
            for conversation in &rows {
                let Some(binding) = self.store.binding(&conversation.binding_id).await? else {
                    continue;
                };
                if let Err(error) = self.reconcile_conversation(&binding, conversation).await {
                    tracing::warn!(binding_id = %binding.binding_id, conversation_id = %conversation.conversation_id, %error, "binding conversation recovery will retry");
                }
            }
            after = rows.last().map(super::store::conversation_storage_key);
        }
        Ok(())
    }

    pub async fn reconcile_binding(&self, binding: &ProviderBinding) -> Result<(), BindingError> {
        use BindingWork::*;
        let mut next = binding.clone();
        match binding.work {
            Idle if super::store::binding_is_recovery_candidate(binding) => {
                return match self.credentials.recover_binding(binding).await {
                    Ok(_) | Err(crate::CoreError::InvalidState(_)) => Ok(()),
                    Err(error) => Err(unavailable(error)),
                };
            }
            AwaitingCredential | Idle => return Ok(()),
            DeliverCredential => {
                let sandbox = self
                    .store
                    .get_sandbox(&binding.sandbox_id)
                    .await?
                    .ok_or(BindingError::NotFound)?;
                if matches!(
                    sandbox.status,
                    SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired
                ) {
                    return self
                        .fail(
                            binding,
                            "binding credential delivery failed or sandbox was retired",
                        )
                        .await;
                }
                if !binding.credential_delivery_complete(&sandbox) {
                    return Ok(());
                }
                next.state = BindingState::Ready;
                next.work = Idle;
            }
            RetirePreviousConversations => {
                let old = binding
                    .previous
                    .as_ref()
                    .ok_or_else(|| BindingError::Corrupt("missing previous environment".into()))?;
                if old.retire_sandbox {
                    self.begin_sandbox_retirement(binding, &old.sandbox_id)
                        .await?;
                }
                if !self.cleanup_conversations(binding, old.generation).await? {
                    return Ok(());
                }
                next.work = RetirePreviousSandbox;
            }
            RetirePreviousSandbox => {
                let old = binding
                    .previous
                    .as_ref()
                    .ok_or_else(|| BindingError::Corrupt("missing previous environment".into()))?;
                if old.retire_sandbox && !self.cleanup_sandbox(binding, &old.sandbox_id).await? {
                    return Ok(());
                }
                next.previous = None;
                next.work = if binding.state == BindingState::Retired {
                    RetireCurrentConversations
                } else if binding.profile.environment.is_some() {
                    ProvisionSandbox
                } else {
                    Idle
                };
            }
            ProvisionSandbox => {
                let profile = self.profiles.sandbox(binding).map_err(unavailable)?;
                let value = match self
                    .resources
                    .sandboxes
                    .create_for_binding(binding, &profile)
                    .await
                {
                    Ok(value) => value,
                    Err(SandboxServiceError::SandboxLimitReached { .. }) => {
                        return self.fail(binding, "sandbox limit reached").await
                    }
                    Err(error) => return Err(unavailable(error)),
                };
                match value.status {
                    SandboxStatus::Active => next.work = ProvisionWorkspace,
                    SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired => {
                        return self
                            .fail(binding, "sandbox provisioning failed or was retired")
                            .await
                    }
                    _ => return Ok(()),
                }
            }
            ProvisionWorkspace => {
                let profile = self.profiles.workspace(binding).map_err(unavailable)?;
                let value = match self
                    .resources
                    .workspaces
                    .create_for_binding(binding, &profile)
                    .await
                {
                    Ok(value) => value,
                    Err(
                        WorkspaceServiceError::LimitReached { .. }
                        | WorkspaceServiceError::NameConflict,
                    ) => {
                        return self
                            .fail(binding, "workspace capacity or name is unavailable")
                            .await
                    }
                    Err(error) => return Err(unavailable(error)),
                };
                match value.status {
                    WorkspaceStatus::Ready => {
                        next.state = BindingState::Ready;
                        next.work = Idle;
                    }
                    WorkspaceStatus::Failed
                    | WorkspaceStatus::Removing
                    | WorkspaceStatus::Removed => {
                        return self
                            .fail(binding, "workspace provisioning failed or was removed")
                            .await
                    }
                    _ => return Ok(()),
                }
            }
            RetireCurrentConversations => {
                if binding.existing_agent.is_none() {
                    self.begin_sandbox_retirement(binding, &binding.sandbox_id)
                        .await?;
                }
                if !self
                    .cleanup_conversations(binding, binding.generation)
                    .await?
                {
                    return Ok(());
                }
                next.work = if binding.existing_agent.is_some() {
                    Idle
                } else {
                    RetireCurrentSandbox
                };
            }
            RetireCurrentSandbox => {
                if !self.cleanup_sandbox(binding, &binding.sandbox_id).await? {
                    return Ok(());
                }
                next.work = Idle;
            }
        }
        next.advance_revision(now_rfc3339())
            .map_err(BindingError::Corrupt)?;
        self.store
            .save_binding(Some(binding.revision), next)
            .await?;
        Ok(())
    }

    async fn fail(&self, binding: &ProviderBinding, reason: &str) -> Result<(), BindingError> {
        let mut next = binding.clone();
        next.state = BindingState::Failed {
            reason: reason.into(),
        };
        next.work = BindingWork::Idle;
        next.advance_revision(now_rfc3339())
            .map_err(BindingError::Corrupt)?;
        self.store
            .save_binding(Some(binding.revision), next)
            .await?;
        Ok(())
    }

    // Fence revival before waiting for paused Agents: their retained runtime must
    // be removed by parent retirement before process absence can be proved.
    // Retirement is monotonic; a concurrent later binding revision cannot make
    // this exact owned hierarchy live again. The parent write also uses its CAS.
    async fn begin_sandbox_retirement(
        &self,
        binding: &ProviderBinding,
        id: &crate::SandboxId,
    ) -> Result<bool, BindingError> {
        if self.store.binding(&binding.binding_id).await?.as_ref() != Some(binding) {
            return Err(BindingError::Conflict);
        }
        self.resources
            .sandboxes
            .retire_for_binding(binding, id)
            .await
            .map_err(unavailable)
    }

    async fn cleanup_sandbox(
        &self,
        binding: &ProviderBinding,
        id: &crate::SandboxId,
    ) -> Result<bool, BindingError> {
        if !self.begin_sandbox_retirement(binding, id).await? {
            return Ok(false);
        }
        // A reservation can be deleted before its Sandbox aggregate exists.
        // Custody retirement uses the same binding fence as credential admission.
        self.credentials
            .forget_binding(binding, id)
            .await
            .map_err(unavailable)?;
        Ok(true)
    }

    async fn cleanup_conversations(
        &self,
        binding: &ProviderBinding,
        generation: u64,
    ) -> Result<bool, BindingError> {
        let mut after = None;
        let mut complete = true;
        loop {
            let rows = self
                .store
                .binding_conversations(&binding.binding_id, generation, after.as_deref(), 200)
                .await?;
            if rows.is_empty() {
                return Ok(complete);
            }
            for value in &rows {
                if value.state != BindingConversationState::Retired {
                    complete = false;
                    self.reconcile_conversation(binding, value).await?;
                }
            }
            after = rows.last().map(|value| value.conversation_id.clone());
        }
    }

    pub async fn reconcile_conversation(
        &self,
        binding: &ProviderBinding,
        value: &BindingConversation,
    ) -> Result<(), BindingError> {
        use BindingConversationState::*;
        if value.state == Retired {
            return Ok(());
        }
        let parent_live = (binding.state == BindingState::Ready
            || binding.work == BindingWork::DeliverCredential)
            && binding.generation == value.generation;
        if (!parent_live || value.state == Retiring) && value.state != Retiring {
            return self.save_conversation_state(binding, value, Retiring).await;
        }
        if value.state == Retiring {
            if binding.existing_agent.is_some()
                || self
                    .resources
                    .agents
                    .retire_for_binding(value)
                    .await
                    .map_err(unavailable)?
            {
                self.save_conversation_state(binding, value, Retired)
                    .await?;
            }
            return Ok(());
        }
        if value.state != Provisioning || binding.work == BindingWork::DeliverCredential {
            return Ok(());
        }
        let profile = self.profiles.agent(binding).map_err(unavailable)?;
        let agent = match self
            .resources
            .agents
            .create_for_binding(binding, value, &profile)
            .await
        {
            Ok(agent) => agent,
            Err(
                AgentServiceError::LimitReached { .. }
                | AgentServiceError::CapabilityUnsupported { .. },
            ) => return self.save_conversation_state(binding, value, Failed).await,
            Err(error) => return Err(unavailable(error)),
        };
        let state = match agent.status {
            AgentStatus::Starting => return Ok(()),
            AgentStatus::Ready
            | AgentStatus::Working
            | AgentStatus::WaitingApproval
            | AgentStatus::Paused => Ready,
            AgentStatus::Stopped | AgentStatus::Failed | AgentStatus::Deleted => Failed,
        };
        self.save_conversation_state(binding, value, state).await
    }

    async fn save_conversation_state(
        &self,
        binding: &ProviderBinding,
        value: &BindingConversation,
        state: BindingConversationState,
    ) -> Result<(), BindingError> {
        let mut next = value.clone();
        next.state = state;
        next.revision = next.revision.checked_add(1).ok_or(BindingError::Conflict)?;
        next.updated_at = now_rfc3339();
        self.store
            .save_binding_conversation(binding.revision, Some(value.revision), next)
            .await?;
        Ok(())
    }
}
