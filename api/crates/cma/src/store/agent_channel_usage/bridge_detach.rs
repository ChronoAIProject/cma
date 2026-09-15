use layer2_core::{
    bridge::{BridgeManagementLease, BridgeWrite},
    resource_access::{ResourceActor, ResourceOwner},
    sandbox::AgentCreationProvenance,
};

use super::*;

/// The enclosing grant revoke/reset proves this exact retained binding. Current
/// management is required; current Agent existence/access is not required to
/// remove that association. No historical identity is used as effect authority.
pub(in crate::store) async fn append_management_detach(
    store: &MongoSandboxStore,
    creation: &AgentCreationProvenance,
    management: &BridgeManagementLease,
    key_sha256: String,
    binding: &ChannelBindingReference,
    reason: ChannelBindingChangeReason,
    session: &mut ClientSession,
) -> Result<ChannelUsageEvent, UsageError> {
    append_detach(
        store,
        creation,
        DetachmentAuthority::Lease(management),
        key_sha256,
        binding,
        reason,
        session,
    )
    .await
}

/// A service-created BridgeWrite is the actual authorized management command.
/// It never constructs a new Caller or borrows the resource owner's identity.
pub(in crate::store) async fn append_transition_detach(
    store: &MongoSandboxStore,
    creation: &AgentCreationProvenance,
    write: &BridgeWrite,
    binding: &ChannelBindingReference,
    reason: ChannelBindingChangeReason,
    session: &mut ClientSession,
) -> Result<ChannelUsageEvent, UsageError> {
    let key = layer2_core::profiles::content_digest(&(
        "cma.bridge.usage.management.v1",
        &write.receipt().id,
        &binding.binding_id,
    ))
    .map_err(|_| conflict())?;
    append_detach(
        store,
        creation,
        DetachmentAuthority::Write(write),
        key,
        binding,
        reason,
        session,
    )
    .await
}

#[derive(Clone, Copy)]
enum DetachmentAuthority<'a> {
    Lease(&'a BridgeManagementLease),
    Write(&'a BridgeWrite),
}
impl<'a> DetachmentAuthority<'a> {
    fn require(
        self,
        owner: &ResourceOwner,
        binding: &ChannelBindingReference,
    ) -> Result<(), UsageError> {
        let record = match self {
            Self::Lease(lease) => {
                lease.require_current().map_err(|_| conflict())?;
                lease.record()
            }
            Self::Write(write) => {
                write.require_current().map_err(|_| conflict())?;
                write.record()
            }
        };
        if &record.owner != owner || record.id.as_str() != binding.bridge_id {
            return Err(conflict());
        }
        Ok(())
    }
    fn actor(self) -> &'a ResourceActor {
        match self {
            Self::Lease(lease) => lease.actor(),
            Self::Write(write) => &write.receipt().receipt.actor,
        }
    }
}

async fn append_detach(
    store: &MongoSandboxStore,
    creation: &AgentCreationProvenance,
    authority: DetachmentAuthority<'_>,
    key_sha256: String,
    binding: &ChannelBindingReference,
    reason: ChannelBindingChangeReason,
    session: &mut ClientSession,
) -> Result<ChannelUsageEvent, UsageError> {
    authority.require(&creation.owner, binding)?;
    let id = &creation.agent_id;
    let current = records::read_retained_state(store, id, session)
        .await?
        .ok_or_else(conflict)?;
    if &current.state.creation != creation {
        return Err(conflict());
    }
    let operation_id = layer2_core::profiles::content_digest(&(
        "channel-usage-operation-v1",
        id,
        &creation.agent_created_at,
        &key_sha256,
    ))
    .map_err(|_| conflict())?;
    if let Some(document) = store
        .channel_usage_events
        .find_one(doc! {"_id":&operation_id})
        .session(&mut *session)
        .await?
    {
        let prior = read_event(&document)?;
        if &prior.before.creation != creation {
            return Err(conflict());
        }
        let expected = plan_retained_channel_detachment(
            &prior.before,
            binding,
            authority.actor(),
            &key_sha256,
            reason,
            &prior.at,
        )?;
        if expected != prior {
            return Err(conflict());
        }
        authority.require(&creation.owner, binding)?;
        return Ok(prior);
    }
    let event = plan_retained_channel_detachment(
        &current.state,
        binding,
        authority.actor(),
        &key_sha256,
        reason,
        &layer2_core::now_rfc3339(),
    )?;
    if event.operation_id != operation_id {
        return Err(conflict());
    }
    store
        .channel_usage_events
        .insert_one(event_document(&event)?)
        .session(&mut *session)
        .await?;
    let next = StoredAgentChannelUsage {
        state: event.after.clone(),
        latest_event_digest: event.digest(),
    };
    let changed = store
        .agent_channel_usage
        .replace_one(
            doc! {"_id":id.as_str(), "revision":current.state.revision as i64},
            state_document(&next)?,
        )
        .session(session)
        .await?;
    if changed.matched_count != 1 {
        return Err(conflict());
    }
    authority.require(&creation.owner, binding)?;
    Ok(event)
}

fn conflict() -> UsageError {
    UsageError::Domain(SandboxStoreError::Conflict)
}
