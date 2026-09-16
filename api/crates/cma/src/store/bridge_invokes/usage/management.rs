use layer2_core::bridge::{BridgePlacementPolicy, BridgeState, BridgeWrite};

use super::*;

mod model;
pub(super) use model::UsageClosure;

pub(in crate::store) async fn apply_management_usage(
    bridges: &MongoBridgeStore,
    previous: &BridgeRecord,
    write: &BridgeWrite,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    write.require_current()?;
    let Some(reason) = transition_reason(previous, write)? else {
        return Ok(());
    };
    let database = bridges.client.database(&bridges.definitions.namespace().db);
    let allocations = majority_collection::<Document>(&database, "bridge_allocations");
    let mut cursor = allocations
        .find(doc! {"bridge_id":previous.id.as_str(),"record.state":{"$ne":"retired"}})
        .sort(doc! {"_id":1_i32})
        .session(&mut *session)
        .await?;
    while cursor.advance(&mut *session).await? {
        write.require_current()?;
        let document = cursor.deserialize_current()?;
        let allocation = crate::store::bridge_allocations::decode_record(&document)
            .map_err(|_| BridgeInvokeError::Corrupt)?;
        if allocation.identity.bridge_id != previous.id || allocation.owner != previous.owner {
            return Err(BridgeInvokeError::TargetMismatch.into());
        }
        if UsageClosure::read(&document, &allocation)?.is_some() {
            // Keep the actual original manager and event identity on replay,
            // including when a later retirement follows an earlier rebind.
            continue;
        }
        let head = BindingHead::read(&document)?;
        if let Some(head) = &head {
            head.require_allocation(&allocation)?;
            if head.linked_grants > 0 {
                let resources = bridges
                    .invoke_resources
                    .get()
                    .ok_or(BridgeInvokeError::Unavailable)?;
                crate::store::agent_channel_usage::bridge_detach::append_transition_detach(
                    resources,
                    &head.creation,
                    write,
                    &head.binding,
                    reason,
                    session,
                )
                .await?;
            }
        }
        let closure = UsageClosure::new(&allocation, previous, write, head.as_ref())?;
        let mut fields = doc! {
            "channel_usage_closure":record_bson(&closure).map_err(|_|BridgeInvokeError::Corrupt)?,
        };
        if let Some(mut next) = head {
            if next.linked_grants > 0 {
                next.revision += 1;
                next.linked_grants = 0;
                fields.insert(
                    "channel_usage_binding",
                    record_bson(&next).map_err(|_| BridgeInvokeError::Corrupt)?,
                );
            }
        }
        let changed = allocations
            .update_one(
                doc! {"_id":&allocation.id,"revision":allocation.revision as i64,
                "channel_usage_closure":{"$exists":false}},
                doc! {"$set":fields},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
    }
    write.require_current()?;
    Ok(())
}

fn transition_reason(
    previous: &BridgeRecord,
    write: &BridgeWrite,
) -> Result<Option<ChannelBindingChangeReason>, BridgeInvokeError> {
    let next = write.record();
    if previous.id != next.id
        || previous.owner != next.owner
        || previous.revision != write.expected_revision()
    {
        return Err(BridgeInvokeError::Conflict);
    }
    if !matches!(previous.state, BridgeState::Retired { .. })
        && matches!(next.state, BridgeState::Retired { .. })
    {
        return Ok(Some(ChannelBindingChangeReason::Detachment));
    }
    let old = &previous.current()?.config;
    let new = &next.current()?.config;
    let explicit_target_change = old.placement != new.placement
        && (matches!(old.placement, BridgePlacementPolicy::ExistingAgent { .. })
            || matches!(new.placement, BridgePlacementPolicy::ExistingAgent { .. }));
    Ok((old.partition != new.partition || explicit_target_change)
        .then_some(ChannelBindingChangeReason::Rebind))
}
