//! One-time managed context refresh. User work always takes precedence.
mod recovery;
use super::agent_operation_dispatch::{
    new_operation, next_sequence, operation_log, runtime_target, PreWriteRollback,
    PreparedAgentOperation,
};
use super::agent_operation_service::{operation_store_error, MAX_OPERATION_COMMIT_RETRIES};
use super::{
    Agent, AgentOperationError, AgentOperationIntent, AgentOperationService, AgentRuntimeTarget,
    AgentWorkAuthority,
};
use crate::{now_rfc3339, AgentId, RuntimeCapability};
use recovery::RefreshRecoveryOnDrop;

fn valid_guidance(guidance: &str) -> bool {
    !guidance.is_empty() && guidance.len() <= 16 * 1024 && !guidance.contains('\0')
}

impl AgentOperationService {
    pub(super) fn managed_context_input_guard(
        &self,
        prepared: &PreparedAgentOperation,
    ) -> std::sync::Arc<dyn layer3_adapter::ConversationInputGuard> {
        std::sync::Arc::new(RefreshInputGuard {
            store: self.store.clone(),
            target: prepared.target.clone(),
            sequence: prepared.operation_sequence,
        })
    }

    /// Internal maintenance: admit only at a settled boundary, with the same
    /// parent/write fences used for owner settings. No synthetic owner message.
    pub async fn refresh_managed_context(
        &self,
        id: &AgentId,
        revision: u64,
        guidance: &str,
        previous_guidance: &[String],
    ) -> Result<bool, AgentOperationError> {
        self.refresh_managed_context_at_origin(id, revision, guidance, previous_guidance, None)
            .await
    }

    pub async fn refresh_managed_context_at_origin(
        &self,
        id: &AgentId,
        revision: u64,
        guidance: &str,
        previous_guidance: &[String],
        service_origin: Option<&str>,
    ) -> Result<bool, AgentOperationError> {
        if revision == 0
            || !valid_guidance(guidance)
            || previous_guidance.len() > 8
            || previous_guidance.iter().any(|p| !valid_guidance(p))
        {
            return Err(AgentOperationError::InvalidRequest(
                "invalid managed context revision".into(),
            ));
        }
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self
                .store
                .get_agent(id)
                .await
                .map_err(operation_store_error)?
                .ok_or(AgentOperationError::NotFound)?;
            if current.conversation.managed_guidance_revision >= revision
                || !super::agent_live_skills::idle(&current)
            {
                return Ok(false);
            }
            let guidance = self
                .instruction_policy
                .pinned_managed_guidance(&current)
                .unwrap_or(guidance);
            if !valid_guidance(guidance) {
                return Err(AgentOperationError::InvalidRequest(
                    "invalid pinned managed context guidance".into(),
                ));
            }
            if current.conversation.operation.as_ref().is_some_and(|operation| {
                matches!(operation.intent,AgentOperationIntent::RefreshContext{revision:prior,..} if prior==revision)
                    && operation.state==super::AgentOperationDelivery::PreWriteFailed
                    && operation.failed_at.as_deref().and_then(crate::seconds_from_rfc3339)
                        .zip(crate::seconds_from_rfc3339(&now_rfc3339()))
                        .is_some_and(|(failed,now)|now.saturating_sub(failed)<30)
            }) { return Ok(false); }
            // Retained profiles without an exact selected model wait for an
            // owner configuration; maintenance must never guess or change it.
            let settings = current.settings();
            if settings.validate().is_err() {
                return Ok(false);
            }
            let (parent, queues) = self.parent_admission(&current).await?;
            if queues {
                return Ok(false);
            }
            let target = runtime_target(&current)?;
            let required = vec![RuntimeCapability::Conversation];
            self.require_runtime(&target, &required).await?;
            let at = now_rfc3339();
            let sequence = next_sequence(current.conversation.last_seq)?;
            let connection = self
                .runtimes
                .resolve(&target)
                .await
                .map_err(|error| AgentOperationError::RuntimeUnavailable(error.to_string()))?;
            let receipt = if service_origin.is_some() {
                connection.input.managed_transport_receipt(sequence).await
            } else {
                connection.input.managed_context_receipt(sequence).await
            };
            let Some(readiness_receipt) = receipt else {
                return Ok(false);
            };
            let intent = AgentOperationIntent::RefreshContext {
                service_origin: service_origin.map(str::to_owned),
                revision,
                readiness_receipt,
                guidance: guidance.into(),
                previous_guidance: previous_guidance.to_vec(),
                settings,
                working_directory: current
                    .conversation
                    .observations
                    .metadata
                    .working_directory
                    .clone(),
            };
            if confirmed_settings(&current, &intent).is_err() {
                return Ok(false);
            }
            let input = refresh_input(&intent, sequence).expect("refresh intent");
            let mut next = current.clone();
            next.conversation.operation =
                Some(new_operation(sequence, &target, sequence, intent, &at)?);
            let logs = vec![operation_log(&next, sequence)?];
            match self
                .commit_admission(&current, next, logs, at, parent)
                .await
            {
                Ok(_) => {
                    let mut recovery = RefreshRecoveryOnDrop {
                        runtimes: self.runtimes.clone(),
                        target: target.clone(),
                        armed: true,
                    };
                    let result = self
                        .dispatch_prepared(PreparedAgentOperation {
                            target,
                            operation_sequence: sequence,
                            turn_sequence: sequence,
                            required,
                            input,
                            rollback: PreWriteRollback::None,
                            queued_message_sequence: None,
                        })
                        .await;
                    recovery.armed = matches!(
                        result,
                        Err(AgentOperationError::DeliveryAmbiguous { .. }
                            | AgentOperationError::OutcomeUnavailable(_))
                    );
                    return result.map(|_| true);
                }
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }

    pub async fn confirm_managed_context_replayed(
        &self,
        target: &AgentRuntimeTarget,
        authority: &AgentWorkAuthority,
        sequence: u64,
    ) -> Result<(), AgentOperationError> {
        self.settle_managed_context_replayed(target, authority, sequence, true)
            .await
    }

    pub async fn retire_managed_context_replayed(
        &self,
        target: &AgentRuntimeTarget,
        authority: &AgentWorkAuthority,
        sequence: u64,
    ) -> Result<(), AgentOperationError> {
        self.settle_managed_context_replayed(target, authority, sequence, false)
            .await
    }

    async fn settle_managed_context_replayed(
        &self,
        target: &AgentRuntimeTarget,
        authority: &AgentWorkAuthority,
        sequence: u64,
        confirmed: bool,
    ) -> Result<(), AgentOperationError> {
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self
                .store
                .get_agent(&target.agent_id)
                .await
                .map_err(operation_store_error)?
                .ok_or(AgentOperationError::NotFound)?;
            let at = now_rfc3339();
            if !current.work.owns(&authority.holder, authority.fence)
                || runtime_target(&current)? != *target
                || current
                    .work
                    .lease
                    .as_ref()
                    .is_none_or(|lease| lease.expires_at <= at)
            {
                return Err(AgentOperationError::StaleWorkAuthority);
            }
            let Some(operation) = current
                .conversation
                .operation
                .as_ref()
                .filter(|op| op.sequence == sequence)
            else {
                return Err(AgentOperationError::Conflict);
            };
            let AgentOperationIntent::RefreshContext { revision, .. } = &operation.intent else {
                return Err(AgentOperationError::Conflict);
            };
            if !operation.blocks_dispatch() {
                return Ok(());
            }
            let mut next = current.clone();
            if confirmed {
                next.conversation.managed_guidance_revision = *revision;
                next.conversation.settings = confirmed_settings(&current, &operation.intent)
                    .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
                next.conversation
                    .operation
                    .as_mut()
                    .expect("checked")
                    .mark_execution_observed(at.clone())
                    .map_err(|e| AgentOperationError::Corrupt(e.to_string()))?;
            } else {
                let failure = super::AgentOperationFailure::new(
                    "managed_context_identity_changed",
                    "retained maintenance authority no longer matches this runtime",
                    at.clone(),
                )
                .map_err(|e| AgentOperationError::Corrupt(e.to_string()))?;
                next.conversation
                    .operation
                    .as_mut()
                    .expect("checked")
                    .retire_lost_runtime(failure)
                    .map_err(|e| AgentOperationError::Corrupt(e.to_string()))?;
            }
            let logs = vec![operation_log(&next, sequence)?];
            match self.commit_change(&current, next, logs, at).await {
                Ok(_) => return Ok(()),
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }
}

impl Agent {
    pub fn pending_managed_context(&self) -> Option<(u64, layer3_adapter::ConversationInput)> {
        let operation = self
            .conversation
            .operation
            .as_ref()
            .filter(|op| op.blocks_dispatch())?;
        Some((
            operation.sequence,
            refresh_input(&operation.intent, operation.sequence)?,
        ))
    }
}

fn refresh_input(
    intent: &AgentOperationIntent,
    operation_sequence: u64,
) -> Option<layer3_adapter::ConversationInput> {
    let AgentOperationIntent::RefreshContext {
        service_origin,
        guidance,
        previous_guidance,
        settings,
        readiness_receipt,
        working_directory,
        ..
    } = intent
    else {
        return None;
    };
    let layer3_adapter::ConversationInput::Configure {
        model,
        reasoning_effort,
        permission,
        ..
    } = settings.runtime_input()
    else {
        unreachable!("settings input");
    };
    Some(layer3_adapter::ConversationInput::RefreshContext {
        service_origin: service_origin.clone(),
        operation_sequence,
        readiness_receipt: readiness_receipt.clone(),
        guidance: guidance.clone(),
        previous_guidance: previous_guidance.clone(),
        model,
        reasoning_effort,
        permission,
        working_directory: working_directory.clone(),
    })
}

pub(super) fn validate_mutation(
    current: &Agent,
    mutation: &super::store::AgentConversationMutation,
) -> Result<(), super::store::SandboxStoreError> {
    use super::store::SandboxStoreError as Error;
    let next = &mutation.agent;
    let admission = next.conversation.operation.as_ref().filter(|op| {
        current
            .conversation
            .operation
            .as_ref()
            .is_none_or(|old| old.sequence != op.sequence)
    });
    if let Some(operation) = admission {
        if let AgentOperationIntent::RefreshContext {
            revision,
            settings,
            working_directory,
            ..
        } = &operation.intent
        {
            if !super::agent_live_skills::idle(current)
                || *revision <= current.conversation.managed_guidance_revision
                || settings != &current.settings()
                || working_directory
                    != &current.conversation.observations.metadata.working_directory
                || mutation.admission_guard.is_none()
                || current.conversation.settings != next.conversation.settings
                || operation.state != super::AgentOperationDelivery::DispatchFenced
            {
                return Err(Error::Conflict);
            }
        }
    }
    if current.conversation.managed_guidance_revision == next.conversation.managed_guidance_revision
    {
        return Ok(());
    }
    let old = current
        .conversation
        .operation
        .as_ref()
        .ok_or(Error::Conflict)?;
    let new = next
        .conversation
        .operation
        .as_ref()
        .ok_or(Error::Conflict)?;
    let AgentOperationIntent::RefreshContext { revision, .. } = &old.intent else {
        return Err(Error::Conflict);
    };
    if !old.blocks_dispatch()
        || old.sequence != new.sequence
        || old.intent != new.intent
        || new.state != super::AgentOperationDelivery::ExecutionObserved
        || *revision != next.conversation.managed_guidance_revision
        || *revision <= current.conversation.managed_guidance_revision
        || mutation.admission_guard.is_some()
        || !valid_settings_confirmation(current, mutation)
    {
        return Err(Error::Conflict);
    }
    Ok(())
}

pub(super) struct RefreshInputGuard {
    pub store: std::sync::Arc<dyn super::AgentOperationStore>,
    pub target: AgentRuntimeTarget,
    pub sequence: u64,
}
#[async_trait::async_trait]
impl layer3_adapter::ConversationInputGuard for RefreshInputGuard {
    async fn check(&self) -> layer3_adapter::Result<()> {
        let refused = || {
            layer3_adapter::AdapterError::Unavailable(
                "managed context authority changed before dispatch".into(),
            )
        };
        let agent = self
            .store
            .get_agent(&self.target.agent_id)
            .await
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        let parent = self
            .store
            .get_sandbox(&agent.sandbox_id)
            .await
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        let mut idle = agent.clone();
        idle.conversation.operation = None;
        if !super::agent_live_skills::idle(&idle)
            || runtime_target(&agent).ok().as_ref() != Some(&self.target)
            || !agent.conversation.operation.as_ref().is_some_and(|op| {
                op.sequence == self.sequence
                    && op.state == super::AgentOperationDelivery::DispatchFenced
                    && matches!(op.intent, AgentOperationIntent::RefreshContext { .. })
            })
            || parent.owner != agent.owner
            || !matches!(
                parent.status,
                super::SandboxStatus::Active | super::SandboxStatus::CredentialRotating
            )
            || parent.lifecycle.checkpoint_capture.is_some()
        {
            return Err(refused());
        }
        Ok(())
    }
}

/// Derive only the managed prefix from the exact retained operation. Owner text,
/// model, effort and permission remain unchanged; replay never uses new policy.
pub(super) fn confirmed_settings(
    agent: &Agent,
    intent: &AgentOperationIntent,
) -> Result<Option<super::AgentSettings>, super::SandboxModelError> {
    let mut configured = agent.conversation.settings.clone();
    let AgentOperationIntent::RefreshContext {
        guidance,
        previous_guidance,
        ..
    } = intent
    else {
        return Err(super::SandboxModelError::invalid_value(
            "agent.managed_context.intent",
        ));
    };
    if let Some(settings) = configured.as_mut() {
        if let Some(original) = settings.system_prompt.as_ref() {
            let owner = previous_guidance
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(guidance.as_str()))
                .filter_map(|prefix| {
                    if original == prefix {
                        Some((prefix.len(), ""))
                    } else {
                        original
                            .strip_prefix(prefix)
                            .and_then(|suffix| suffix.strip_prefix("\n\n"))
                            .map(|owner| (prefix.len(), owner))
                    }
                })
                .max_by_key(|(length, _)| *length)
                .map_or(original.as_str(), |(_, owner)| owner);
            settings.system_prompt = Some(if owner.is_empty() {
                guidance.clone()
            } else {
                format!("{guidance}\n\n{owner}")
            });
            settings.validate()?;
        }
    }
    Ok(configured)
}

pub(super) fn valid_settings_confirmation(
    current: &Agent,
    mutation: &super::store::AgentConversationMutation,
) -> bool {
    let Some(old) = current.conversation.operation.as_ref() else {
        return false;
    };
    let Some(new) = mutation.agent.conversation.operation.as_ref() else {
        return false;
    };
    let AgentOperationIntent::RefreshContext { revision, .. } = &old.intent else {
        return false;
    };
    old.blocks_dispatch()
        && old.sequence == new.sequence
        && old.intent == new.intent
        && new.state == super::AgentOperationDelivery::ExecutionObserved
        && mutation.admission_guard.is_none()
        && mutation.agent.conversation.managed_guidance_revision == *revision
        && confirmed_settings(current, &old.intent).ok().as_ref()
            == Some(&mutation.agent.conversation.settings)
}
