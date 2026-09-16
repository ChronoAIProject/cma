//! Retain the exact accepted control outcome, including terminal refusals.
use super::inventory::{project, require_reference, rows, InventoryRow, MAX_ROWS};
use super::*;
use crate::store::bridge_invokes::{channel_controls::records, outbox::control_context};
use layer2_core::bridge::channel_control::ChannelControlHold;
use std::collections::BTreeMap;

pub(super) async fn command(
    database: &Database,
    allocation: &AllocationRecord,
    row: &Document,
    session: &mut ClientSession,
) -> Result<Result<(), HistoryHold>, MutationError> {
    let admitted = records::read_admitted(row).map_err(|_| AllocationError::Corrupt)?;
    admitted
        .sidecar
        .intent
        .reference
        .binding()
        .require_retained_allocation(allocation)
        .map_err(|_| AllocationError::IdentityMismatch)?;
    if !admitted.command.status.is_terminal()
        || admitted.command.work.lease.is_some()
        || admitted.sidecar.execution_hold == ChannelControlHold::WorkerUnavailable
        || admitted
            .command
            .work
            .observation
            .as_ref()
            .is_some_and(|value| {
                value.delivery == layer2_core::sandbox::AgentOperationDelivery::RecoveryRequired
            })
    {
        return Ok(Err(HistoryHold::ControlHistoryUnresolved));
    }
    let Some(intent) = majority_collection::<Document>(database, "bridge_channel_control_intents")
        .find_one(doc! {"_id": &admitted.sidecar.intent.id})
        .session(&mut *session)
        .await?
    else {
        return Ok(Err(HistoryHold::ReferencedRecordMissing));
    };
    if records::read_intent(&intent).map_err(|_| AllocationError::Corrupt)?
        != admitted.sidecar.intent
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    if !require_reference(
        database,
        allocation,
        ReferenceKind::ControlCommand,
        row,
        session,
    )
    .await?
    {
        return Ok(Err(HistoryHold::ReferenceAccountingIncomplete));
    }
    Ok(Ok(()))
}

pub(crate) async fn capture(
    database: &Database,
    allocation: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<Result<Vec<InventoryRow>, HistoryHold>, MutationError> {
    let mut inventory = BTreeMap::new();
    let mut references = BTreeMap::new();
    let reference_rows = rows(
        database,
        "bridge_channel_control_references",
        filter(allocation, "record.binding.source"),
        session,
    )
    .await?;
    let intents = rows(
        database,
        "bridge_channel_control_intents",
        filter(allocation, "record.reference.binding.source"),
        session,
    )
    .await?;
    if reference_rows.len() > MAX_ROWS as usize || intents.len() > MAX_ROWS as usize {
        return Ok(Err(HistoryHold::InventoryBoundExceeded));
    }
    for row in reference_rows {
        let reference =
            control_context::read_reference(&row).map_err(|_| AllocationError::Corrupt)?;
        reference
            .binding()
            .require_retained_allocation(allocation)
            .map_err(|_| AllocationError::IdentityMismatch)?;
        if !require_reference(
            database,
            allocation,
            ReferenceKind::ControlReference,
            &row,
            session,
        )
        .await?
        {
            return Ok(Err(HistoryHold::ReferenceAccountingIncomplete));
        }
        references.insert(reference.reference.clone(), reference);
        insert(&mut inventory, "bridge_channel_control_references", &row)?;
    }
    for row in intents {
        let intent = records::read_intent(&row).map_err(|_| AllocationError::Corrupt)?;
        if references.get(&intent.reference.reference) != Some(&intent.reference) {
            return Ok(Err(HistoryHold::ReferencedRecordMissing));
        }
        if !require_reference(
            database,
            allocation,
            ReferenceKind::ControlIntent,
            &row,
            session,
        )
        .await?
        {
            return Ok(Err(HistoryHold::ReferenceAccountingIncomplete));
        }
        insert(&mut inventory, "bridge_channel_control_intents", &row)?;
        let Some(command_row) = majority_collection::<Document>(database, "agent_commands")
            .find_one(
                doc! {"_id": intent.command_id().map_err(|_| AllocationError::Corrupt)?.as_str()},
            )
            .session(&mut *session)
            .await?
        else {
            return Ok(Err(HistoryHold::ReferencedRecordMissing));
        };
        if let Err(hold) = command(database, allocation, &command_row, session).await? {
            return Ok(Err(hold));
        }
        let admitted =
            records::read_admitted(&command_row).map_err(|_| AllocationError::Corrupt)?;
        if admitted.sidecar.intent != intent {
            return Err(AllocationError::IdentityMismatch.into());
        }
        // Include the terminal execution sidecar as well as the common command.
        inventory.insert(
            (
                "agent_commands".into(),
                admitted.sidecar.command_id.as_str().into(),
            ),
            InventoryRow {
                collection: "agent_commands".into(),
                id: admitted.sidecar.command_id.as_str().into(),
                sha256: digest(
                    command_row
                        .get("channel_control_admission")
                        .ok_or(AllocationError::Corrupt)?,
                )?,
            },
        );
        let aliases = rows(
            database,
            "bridge_channel_control_aliases",
            doc! {"record.intent_id": &intent.id},
            session,
        )
        .await?;
        if aliases.len() > MAX_ROWS as usize {
            return Ok(Err(HistoryHold::InventoryBoundExceeded));
        }
        let mut original = false;
        for alias in aliases {
            let callback =
                records::history_alias(&alias, &intent).map_err(|_| AllocationError::Corrupt)?;
            original |= callback == intent.callback;
            if !require_reference(
                database,
                allocation,
                ReferenceKind::ControlAlias,
                &alias,
                session,
            )
            .await?
            {
                return Ok(Err(HistoryHold::ReferenceAccountingIncomplete));
            }
            insert(&mut inventory, "bridge_channel_control_aliases", &alias)?;
            let dispositions = match crate::store::MongoCallbackInbox::control_history_documents(
                database, &intent, &callback, session,
            )
            .await
            {
                Ok(rows) => rows,
                Err(crate::store::bridge_definitions::inbox::MutationError::Mongo(error)) => {
                    return Err(error.into())
                }
                Err(_) => return Ok(Err(HistoryHold::ControlHistoryUnresolved)),
            };
            for disposition in dispositions {
                if !require_reference(
                    database,
                    allocation,
                    ReferenceKind::ControlDisposition,
                    &disposition,
                    session,
                )
                .await?
                {
                    return Ok(Err(HistoryHold::ReferenceAccountingIncomplete));
                }
                insert(&mut inventory, "bridge_callback_dispositions", &disposition)?;
            }
        }
        if !original {
            return Ok(Err(HistoryHold::ReferencedRecordMissing));
        }
        // Generic callback replay can retain a disposition before the dedicated
        // control store adds its optional alias. Inventory both entry points.
        let dispositions = rows(
            database,
            "bridge_callback_dispositions",
            doc! {"record.receipt.receipt.intent_id": &intent.id},
            session,
        )
        .await?;
        if dispositions.len() > MAX_ROWS as usize {
            return Ok(Err(HistoryHold::InventoryBoundExceeded));
        }
        for disposition in dispositions {
            crate::store::MongoCallbackInbox::validate_control_history_disposition(
                &disposition,
                &intent,
            )
            .map_err(|_| AllocationError::Corrupt)?;
            if !require_reference(
                database,
                allocation,
                ReferenceKind::ControlDisposition,
                &disposition,
                session,
            )
            .await?
            {
                return Ok(Err(HistoryHold::ReferenceAccountingIncomplete));
            }
            insert(&mut inventory, "bridge_callback_dispositions", &disposition)?;
        }
        if inventory.len() > MAX_ROWS as usize {
            return Ok(Err(HistoryHold::InventoryBoundExceeded));
        }
    }
    Ok(Ok(inventory.into_values().collect()))
}
fn filter(allocation: &AllocationRecord, prefix: &str) -> Document {
    let mut root = Document::new();
    root.insert(format!("{prefix}.allocation_id"), &allocation.id);
    let mut agent = Document::new();
    agent.insert(
        format!("{prefix}.target.agent_id"),
        allocation.placement.target().agent_id.as_str(),
    );
    doc! {"$or": [root, agent]}
}
fn insert(
    inventory: &mut BTreeMap<(String, String), InventoryRow>,
    collection: &str,
    row: &Document,
) -> Result<(), AllocationError> {
    let projected = project(collection, row)?;
    inventory.insert(
        (projected.collection.clone(), projected.id.clone()),
        projected,
    );
    Ok(())
}
