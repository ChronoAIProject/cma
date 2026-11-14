use super::*;

/// Detachment removes a proven historical association. It never invokes or
/// modifies the Agent, and must remain possible when its resource was removed
/// or recreated. The Bridge adapter must fence current management authority and
/// the exact retained grant/allocation link in the same transaction.
pub fn plan_retained_channel_detachment(
    before: &AgentChannelUsageState,
    binding: &ChannelBindingReference,
    actor: &ResourceActor,
    key_sha256: &str,
    reason: ChannelBindingChangeReason,
    at: &str,
) -> Result<ChannelUsageEvent, SandboxStoreError> {
    before.validate()?;
    binding.validate()?;
    if !digest(key_sha256)
        || !valid_actor(actor)
        || before
            .active_bindings
            .iter()
            .all(|active| &active.reference != binding)
    {
        return Err(SandboxStoreError::Conflict);
    }
    let change = ChannelUsageChange::Detach {
        binding_id: binding.binding_id.clone(),
    };
    let after = transition::apply(before, &change, at, reason, None)?;
    let event = ChannelUsageEvent {
        operation_id: hash(&(
            "channel-usage-operation-v1",
            &before.creation.agent_id,
            &before.creation.agent_created_at,
            key_sha256,
        )),
        fingerprint: hash(&(
            "channel-usage-input-v1",
            &before.creation,
            before.revision,
            actor,
            &change,
            reason,
        )),
        actor: actor.clone(),
        at: at.into(),
        detached_at: Some(at.into()),
        reason,
        change,
        predecessor: None,
        before: before.clone(),
        after,
        retained_digest: None,
    };
    event.validate()?;
    Ok(event)
}
