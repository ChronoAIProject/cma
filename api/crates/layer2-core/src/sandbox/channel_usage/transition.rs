use super::*;
use crate::sandbox::store::AgentProvenanceSnapshot;

/// Internal server input. Its policy fence prevents a stored decision being
/// used after sharing/ownership changes; bridge/grant authority must separately
/// be established and fenced by the calling bridge transaction.
#[derive(Debug, Clone)]
pub struct ChannelUsageMutation {
    pub creation: AgentCreationProvenance,
    pub policy_fence: ResourcePolicyFence,
    pub expected_revision: u64,
    pub key_sha256: String,
    pub actor: ResourceActor,
    pub change: ChannelUsageChange,
    pub reason: ChannelBindingChangeReason,
    pub at: String,
}

impl ChannelUsageMutation {
    pub fn operation_id(&self) -> String {
        hash(&(
            "channel-usage-operation-v1",
            &self.creation.agent_id,
            &self.creation.agent_created_at,
            &self.key_sha256,
        ))
    }
    pub fn fingerprint(&self) -> String {
        hash(&(
            "channel-usage-input-v1",
            &self.creation,
            self.expected_revision,
            &self.actor,
            &self.change,
            self.reason,
        ))
    }
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if !digest(&self.key_sha256)
            || !valid_actor(&self.actor)
            || self.expected_revision >= i64::MAX as u64
            || !ordered(&self.creation.agent_created_at, &self.at)
            || self.policy_fence.sandbox_id != self.creation.sandbox_id
            || self.policy_fence.sandbox_created_at != self.creation.sandbox_created_at
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn require_current(
        &self,
        snapshot: &AgentProvenanceSnapshot,
    ) -> Result<(), SandboxStoreError> {
        self.validate()?;
        if snapshot.creation_provenance(&self.creation.agent_id)? != self.creation
            || snapshot.resource.fence() != self.policy_fence
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
    pub fn replay(&self, event: ChannelUsageEvent) -> Result<ChannelUsageEvent, SandboxStoreError> {
        event.validate()?;
        if event.operation_id != self.operation_id()
            || event.fingerprint != self.fingerprint()
            || event.before.creation != self.creation
            || event.before.revision != self.expected_revision
            || event.actor != self.actor
            || event.change != self.change
            || event.reason != self.reason
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(event)
    }
}

pub fn plan_channel_usage(
    request: &ChannelUsageMutation,
    snapshot: &AgentProvenanceSnapshot,
    before: &AgentChannelUsageState,
    predecessor: Option<ChannelBindingReference>,
) -> Result<ChannelUsageEvent, SandboxStoreError> {
    request.require_current(snapshot)?;
    if !matches!(request.change, ChannelUsageChange::Detach { .. })
        && (snapshot.resource.lineage.sandbox.retired_at.is_some()
            || snapshot
                .resource
                .lineage
                .agent
                .as_ref()
                .is_none_or(|agent| agent.status == crate::sandbox::AgentStatus::Deleted))
    {
        return Err(SandboxStoreError::Conflict);
    }
    before.validate()?;
    if before.creation != request.creation || before.revision != request.expected_revision {
        return Err(SandboxStoreError::Conflict);
    }
    let after = apply(
        before,
        &request.change,
        &request.at,
        request.reason,
        predecessor.as_ref(),
    )?;
    let event = ChannelUsageEvent {
        operation_id: request.operation_id(),
        fingerprint: request.fingerprint(),
        actor: request.actor.clone(),
        at: request.at.clone(),
        detached_at: matches!(
            request.change,
            ChannelUsageChange::Detach { .. } | ChannelUsageChange::Replace { .. }
        )
        .then(|| request.at.clone()),
        reason: request.reason,
        change: request.change.clone(),
        predecessor,
        before: before.clone(),
        after,
    };
    event.validate()?;
    Ok(event)
}

pub(super) fn apply(
    before: &AgentChannelUsageState,
    change: &ChannelUsageChange,
    at: &str,
    reason: ChannelBindingChangeReason,
    predecessor: Option<&ChannelBindingReference>,
) -> Result<AgentChannelUsageState, SandboxStoreError> {
    before.validate()?;
    if !ordered(
        before
            .last_changed_at
            .as_deref()
            .unwrap_or(&before.creation.agent_created_at),
        at,
    ) {
        return Err(invalid());
    }
    let mut after = before.clone();
    after.revision = before
        .revision
        .checked_add(1)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or(SandboxStoreError::Conflict)?;
    after.last_changed_at = Some(at.into());
    match change {
        ChannelUsageChange::Attach { binding } | ChannelUsageChange::Replace { binding, .. } => {
            binding.validate()?;
            match (binding.previous_binding_id.as_ref(), predecessor) {
                (None, None) => {}
                (Some(id), Some(previous))
                    if id == &previous.binding_id
                        && binding.bridge_id == previous.bridge_id
                        && binding.partition_id == previous.partition_id
                        && previous.generation.checked_add(1) == Some(binding.generation) => {}
                _ => return Err(SandboxStoreError::Conflict),
            }
            if let Some(previous) = predecessor {
                previous.validate()?;
            }
            if after
                .active_bindings
                .iter()
                .any(|b| b.reference.binding_id == binding.binding_id)
            {
                return Err(SandboxStoreError::Conflict);
            }
            if let ChannelUsageChange::Replace {
                before_binding_id, ..
            } = change
            {
                if !matches!(
                    reason,
                    ChannelBindingChangeReason::Reset
                        | ChannelBindingChangeReason::Rebind
                        | ChannelBindingChangeReason::AdministrativeIntervention
                ) || binding.previous_binding_id.as_ref() != Some(before_binding_id)
                {
                    return Err(invalid());
                }
                let position = after
                    .active_bindings
                    .iter()
                    .position(|b| &b.reference.binding_id == before_binding_id)
                    .ok_or(SandboxStoreError::Conflict)?;
                if predecessor != Some(&after.active_bindings[position].reference) {
                    return Err(SandboxStoreError::Conflict);
                }
                after.active_bindings.remove(position);
            } else if !matches!(
                reason,
                ChannelBindingChangeReason::Attachment
                    | ChannelBindingChangeReason::AdministrativeIntervention
            ) {
                return Err(invalid());
            }
            if after.active_bindings.len() >= MAX_ACTIVE_CHANNEL_BINDINGS {
                return Err(invalid());
            }
            if after.active_bindings.iter().any(|b| {
                b.reference.bridge_id == binding.bridge_id
                    && b.reference.partition_id == binding.partition_id
            }) {
                return Err(SandboxStoreError::Conflict);
            }
            after.active_bindings.push(ActiveChannelBinding {
                reference: binding.clone(),
                attached_at: at.into(),
                first_used_at: None,
                last_used_at: None,
            });
        }
        ChannelUsageChange::Use { binding_id } | ChannelUsageChange::Detach { binding_id } => {
            if !opaque(binding_id) {
                return Err(invalid());
            }
            let position = after
                .active_bindings
                .iter()
                .position(|b| &b.reference.binding_id == binding_id);
            if matches!(change, ChannelUsageChange::Use { .. }) {
                if reason != ChannelBindingChangeReason::AcceptedCommand {
                    return Err(invalid());
                }
                if let Some(position) = position {
                    if predecessor.is_some() {
                        return Err(invalid());
                    }
                    let active = &mut after.active_bindings[position];
                    active.first_used_at.get_or_insert(at.into());
                    active.last_used_at = Some(at.into());
                } else {
                    // DrainOriginal may admit an already accepted callback after
                    // management detached its allocation. Exact retained binding
                    // evidence records historical use without restoring activity.
                    let binding = predecessor.ok_or(SandboxStoreError::Conflict)?;
                    binding.validate()?;
                    if &binding.binding_id != binding_id {
                        return Err(SandboxStoreError::Conflict);
                    }
                }
                after.first_used_at.get_or_insert(at.into());
                after.last_used_at = Some(at.into());
            } else {
                if !matches!(
                    reason,
                    ChannelBindingChangeReason::Detachment
                        | ChannelBindingChangeReason::Reset
                        | ChannelBindingChangeReason::Rebind
                        | ChannelBindingChangeReason::AdministrativeIntervention
                ) {
                    return Err(invalid());
                }
                if predecessor.is_some() {
                    return Err(invalid());
                }
                after
                    .active_bindings
                    .remove(position.ok_or(SandboxStoreError::Conflict)?);
            }
        }
    }
    after
        .active_bindings
        .sort_by(|a, b| a.reference.binding_id.cmp(&b.reference.binding_id));
    after.validate()?;
    Ok(after)
}
