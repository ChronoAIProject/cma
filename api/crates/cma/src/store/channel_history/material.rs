use super::inventory::{project, require_reference, rows, InventoryRow, MAX_ROWS};
use super::*;
use crate::sandbox::channel_retirement::model::ChannelRetirementSnapshot;

pub(super) async fn commands(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    session: &mut ClientSession,
) -> Result<Result<Vec<InventoryRow>, HistoryHold>, MutationError> {
    let agent_id = snapshot.allocation.placement.target().agent_id.as_str();
    let rows = rows(
        database,
        "agent_commands",
        doc! {"$or": [
            {"agent_id": agent_id}, {"record.receipt.envelope.agent_id": agent_id},
            {"channel_admission.allocation_id": &snapshot.allocation.id}
        ]},
        session,
    )
    .await?;
    if rows.len() > MAX_ROWS as usize {
        return Ok(Err(HistoryHold::InventoryBoundExceeded));
    }
    let mut orders = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    for row in &rows {
        let command = crate::store::agent_commands::read_command(row)
            .map_err(|_| AllocationError::Corrupt)?;
        if command.receipt.envelope.agent_id.as_str() != agent_id
            || !orders.insert(command.receipt.agent_order)
        {
            return Err(AllocationError::IdentityMismatch.into());
        }
        if !command.status.is_terminal() || command.work.lease.is_some() {
            return Ok(Err(HistoryHold::AcceptedWorkPending));
        }
        if row.contains_key("channel_admission") {
            for kind in [
                ReferenceKind::AcceptedCommand,
                ReferenceKind::AcceptedSource,
            ] {
                if !require_reference(database, &snapshot.allocation, kind, row, session).await? {
                    return Ok(Err(HistoryHold::ReferenceAccountingIncomplete));
                }
            }
        }
        if row.contains_key("channel_control_admission") {
            if let Err(hold) =
                controls::command(database, &snapshot.allocation, row, session).await?
            {
                return Ok(Err(hold));
            }
        }
        result.push(project("agent_commands", row)?);
    }
    let lane = majority_collection::<Document>(database, "agent_command_orders")
        .find_one(doc! {"_id": agent_id})
        .session(&mut *session)
        .await?;
    let last = match lane.as_ref() {
        Some(row) => {
            let lane = crate::store::agent_commands::read_command_lane(row, agent_id)
                .map_err(|_| AllocationError::Corrupt)?;
            if lane.lease.is_some() {
                return Ok(Err(HistoryHold::AcceptedWorkPending));
            }
            lane.last_order
        }
        None if rows.is_empty() => 0,
        None => return Err(AllocationError::Corrupt.into()),
    };
    if last != rows.len() as u64 || orders.into_iter().ne(1..=last) {
        return Err(AllocationError::Corrupt.into());
    }
    Ok(Ok(result))
}

pub(crate) async fn outbox(
    database: &Database,
    allocation: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<Result<Vec<InventoryRow>, HistoryHold>, MutationError> {
    let rows = rows(
        database,
        "bridge_outbox_items",
        doc! {"$or": [
            {"record.source.allocation_id": &allocation.id},
            {"record.source.target.agent_id": allocation.placement.target().agent_id.as_str()},
            {"record.source.target.sandbox_id": allocation.placement.target().sandbox_id.as_str()}
        ]},
        session,
    )
    .await?;
    if rows.len() > MAX_ROWS as usize {
        return Ok(Err(HistoryHold::InventoryBoundExceeded));
    }
    let mut result = Vec::new();
    for row in rows {
        let item = crate::store::bridge_invokes::outbox::read_cleanup_item(&row)
            .map_err(|_| AllocationError::Corrupt)?;
        if item.source.allocation_id() != allocation.id {
            return Err(AllocationError::IdentityMismatch.into());
        }
        // Preserve the actual audited terminal outcome; finalization freezes
        // later retry mutations without relabeling abandonment as delivery.
        if !matches!(
            item.state,
            layer2_core::bridge::outbox::DeliveryState::Sent { .. }
                | layer2_core::bridge::outbox::DeliveryState::Rejected { .. }
                | layer2_core::bridge::outbox::DeliveryState::Abandoned { .. }
        ) {
            return Ok(Err(HistoryHold::OutboxUnresolved));
        }
        if !require_reference(
            database,
            allocation,
            ReferenceKind::OutboxItem,
            &row,
            session,
        )
        .await?
        {
            return Ok(Err(HistoryHold::ReferenceAccountingIncomplete));
        }
        let audit = majority_collection::<Document>(database, "bridge_outbox_audits")
            .find_one(doc! {"_id": format!("{}:{}", item.id, item.revision)})
            .session(&mut *session)
            .await?;
        let Some(audit) = audit else {
            return Ok(Err(HistoryHold::ReferencedRecordMissing));
        };
        if audit.get("record") != row.get("record")
            || audit.get_str("item_id").ok() != Some(item.id.as_str())
            || audit.get_i64("revision").ok() != Some(item.revision as i64)
        {
            return Err(AllocationError::IdentityMismatch.into());
        }
        result.push(project("bridge_outbox_items", &row)?);
    }
    Ok(Ok(result))
}
