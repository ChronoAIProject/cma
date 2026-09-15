use layer2_core::{bridge::BridgeManagementLease, sandbox::AgentCreationProvenance};

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
    management.require_current().map_err(|_| conflict())?;
    if management.record().owner != creation.owner
        || management.record().id.as_str() != binding.bridge_id
    {
        return Err(conflict());
    }
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
            management.actor(),
            &key_sha256,
            reason,
            &prior.at,
        )?;
        if expected != prior {
            return Err(conflict());
        }
        management.require_current().map_err(|_| conflict())?;
        return Ok(prior);
    }
    let event = plan_retained_channel_detachment(
        &current.state,
        binding,
        management.actor(),
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
    management.require_current().map_err(|_| conflict())?;
    Ok(event)
}

fn conflict() -> UsageError {
    UsageError::Domain(SandboxStoreError::Conflict)
}
