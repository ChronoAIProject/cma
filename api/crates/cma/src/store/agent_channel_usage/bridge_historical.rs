//! A real command accepted under DrainOriginal updates historical use only.
//! The caller fences the exact grant/allocation and immutable closure receipt.
use super::bridge_append::FencedUsageActor;
use super::*;

pub(in crate::store) async fn append_historical_use(
    store: &MongoSandboxStore,
    id: &AgentId,
    authority: FencedUsageActor<'_>,
    key_sha256: String,
    binding: &ChannelBindingReference,
    session: &mut ClientSession,
) -> Result<ChannelUsageEvent, UsageError> {
    let FencedUsageActor::Accepted { reservation, .. } = authority else {
        return Err(SandboxStoreError::Conflict.into());
    };
    let snapshot = read_snapshot(store, id, session).await?;
    let (actor, bridge_id) = authority.identity(&snapshot)?;
    if binding.bridge_id != bridge_id.as_str()
        || binding.partition_id != reservation.allocation().identity.key.hash()
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    let mut request = ChannelUsageMutation {
        creation: snapshot.state.creation.clone(),
        policy_fence: snapshot.provenance.resource.fence(),
        expected_revision: snapshot.state.revision,
        key_sha256,
        actor,
        change: ChannelUsageChange::Use {
            binding_id: binding.binding_id.clone(),
        },
        reason: ChannelBindingChangeReason::AcceptedCommand,
        at: layer2_core::now_rfc3339(),
    };
    request.validate()?;
    let prior = store
        .channel_usage_events
        .find_one(doc! {"_id":request.operation_id()})
        .session(&mut *session)
        .await?
        .as_ref()
        .map(read_event)
        .transpose()?;
    let before = if let Some(prior) = &prior {
        request.expected_revision = prior.before.revision;
        request.at = prior.at.clone();
        request.replay(prior.clone())?;
        &prior.before
    } else {
        &snapshot.state
    };
    let event = plan_channel_usage(
        &request,
        &snapshot.provenance,
        before,
        Some(binding.clone()),
    )?;
    if let Some(prior) = prior {
        if prior != event {
            return Err(SandboxStoreError::Conflict.into());
        }
    } else {
        crate::store::resource_access::fence_lineage(store, &snapshot.provenance.resource, session)
            .await?;
        store
            .channel_usage_events
            .insert_one(event_document(&event)?)
            .session(&mut *session)
            .await?;
        let changed = store
            .agent_channel_usage
            .replace_one(
                doc! {"_id":id.as_str(),"revision":snapshot.state.revision as i64},
                state_document(&StoredAgentChannelUsage {
                    state: event.after.clone(),
                    latest_event_digest: event.digest(),
                })?,
            )
            .upsert(snapshot.state.revision == 0)
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 && changed.upserted_id.is_none() {
            return Err(SandboxStoreError::Conflict.into());
        }
    }
    authority.identity(&snapshot)?;
    Ok(event)
}
