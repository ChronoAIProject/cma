use super::*;
use layer2_core::{bridge::outbox::DeliveryState, sandbox::AgentOperationDelivery};

pub(super) async fn holds(
    database: &Database,
    allocation: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<BTreeSet<ChannelReuseCleanupHold>, MutationError> {
    inspect(database, allocation, false, session).await
}

pub(super) async fn fence_holds(
    database: &Database,
    allocation: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<BTreeSet<ChannelReuseCleanupHold>, MutationError> {
    inspect(database, allocation, true, session).await
}

async fn inspect(
    database: &Database,
    allocation: &AllocationRecord,
    write_fence: bool,
    session: &mut ClientSession,
) -> Result<BTreeSet<ChannelReuseCleanupHold>, MutationError> {
    let target = allocation.placement.target();
    let mut holds = BTreeSet::new();
    let mut response_ids = Vec::new();
    let commands: Collection<Document> = majority_collection(database, "agent_commands");
    let mut cursor = commands
        .find(doc! {"$or":[{"agent_id":target.agent_id.as_str()},
        {"record.receipt.envelope.agent_id":target.agent_id.as_str()}]})
        .sort(doc! {"_id":1})
        .limit(257)
        .session(&mut *session)
        .await?;
    let mut count = 0;
    while cursor.advance(&mut *session).await? {
        count += 1;
        if count > 256 {
            holds.insert(ChannelReuseCleanupHold::InventoryLimit);
            break;
        }
        let row = cursor.deserialize_current()?;
        let command = crate::store::agent_commands::read_command(&row)
            .map_err(|_| AllocationError::Corrupt)?;
        if command.receipt.envelope.agent_id != target.agent_id
            || command.receipt.envelope.owner != allocation.owner
        {
            return Err(AllocationError::IdentityMismatch.into());
        }
        if let Some(response_id) = &command.receipt.response_id {
            response_ids.push(response_id.as_str().to_owned());
        }
        if !command.status.is_terminal() {
            holds.insert(ChannelReuseCleanupHold::PendingCommand);
        }
        let delivery = command
            .work
            .observation
            .as_ref()
            .map(|value| value.delivery)
            .or_else(|| command.work.dispatch.as_ref().map(|value| value.delivery));
        if matches!(
            delivery,
            Some(
                AgentOperationDelivery::DispatchFenced
                    | AgentOperationDelivery::RecoveryRequired
                    | AgentOperationDelivery::RuntimeLost
            )
        ) {
            holds.insert(ChannelReuseCleanupHold::UncertainCommand);
        }
        if write_fence {
            fence_row(database, "agent_commands", row, session).await?;
        }
    }
    let replies: Collection<Document> = majority_collection(database, "bridge_outbox_items");
    let mut cursor = replies
        .find(
            doc! {"$or":[{"record.source.target.agent_id":target.agent_id.as_str()},
            {"record.source.allocation_id":&allocation.id}, {"response_id":{"$in":&response_ids}},
            {"record.source.response_id":{"$in":&response_ids}}]},
        )
        .sort(doc! {"_id":1})
        .limit(257)
        .session(&mut *session)
        .await?;
    let mut count = 0;
    while cursor.advance(&mut *session).await? {
        count += 1;
        if count > 256 {
            holds.insert(ChannelReuseCleanupHold::InventoryLimit);
            break;
        }
        let row = cursor.deserialize_current()?;
        let item = crate::store::bridge_invokes::outbox::read_cleanup_item(&row)
            .map_err(|_| AllocationError::Corrupt)?;
        if item.source.target() != target || item.source.owner() != &allocation.owner {
            return Err(AllocationError::IdentityMismatch.into());
        }
        match item.state {
            DeliveryState::Sent { .. }
            | DeliveryState::Rejected { .. }
            | DeliveryState::Abandoned { .. } => {}
            DeliveryState::Unknown { .. } => {
                holds.insert(ChannelReuseCleanupHold::UncertainReply);
            }
            _ => {
                holds.insert(ChannelReuseCleanupHold::PendingReply);
            }
        }
        if write_fence {
            fence_row(database, "bridge_outbox_items", row, session).await?;
        }
    }
    // Only additive observation fences may change. Settlement or abandonment
    // still requires the existing command/outbox recovery authority.
    Ok(holds)
}
