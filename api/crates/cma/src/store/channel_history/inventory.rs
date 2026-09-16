use super::*;
use crate::sandbox::channel_retirement::model::ChannelRetirementSnapshot;
use serde::{Deserialize, Serialize};

pub(super) const MAX_ROWS: i64 = 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InventoryRow {
    pub collection: String,
    pub id: String,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MaterialInventory {
    pub agent_conversation_sha256: String,
    pub log_count: u64,
    pub log_sha256: String,
    pub commands: Vec<InventoryRow>,
    pub outbox: Vec<InventoryRow>,
    pub ancillary: Vec<InventoryRow>,
}

pub(super) async fn capture(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    session: &mut ClientSession,
) -> Result<Result<MaterialInventory, HistoryHold>, MutationError> {
    let target = snapshot.allocation.placement.target();
    for (collection, prefix) in [
        (
            "bridge_channel_control_intents",
            "record.reference.binding.source",
        ),
        ("bridge_channel_control_references", "record.binding.source"),
    ] {
        let mut allocation = Document::new();
        allocation.insert(format!("{prefix}.allocation_id"), &snapshot.allocation.id);
        let mut agent = Document::new();
        agent.insert(
            format!("{prefix}.target.agent_id"),
            target.agent_id.as_str(),
        );
        if majority_collection::<Document>(database, collection)
            .find_one(doc! {"$or": [allocation, agent]})
            .session(&mut *session)
            .await?
            .is_some()
        {
            return Ok(Err(HistoryHold::ControlHistoryUnresolved));
        }
    }
    let agent_row = majority_collection::<Document>(database, "agents")
        .find_one(doc! {"_id": target.agent_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Corrupt)?;
    let agent =
        crate::store::agents::read_agent(&agent_row).map_err(|_| AllocationError::Corrupt)?;
    if agent.status.is_live()
        || agent.conversation.active_turn.is_some()
        || agent.work.lease.is_some()
        || !agent.work.cleanup_debt.is_empty()
        || agent.work.step != layer2_core::sandbox::AgentWorkStep::None
    {
        return Ok(Err(HistoryHold::AcceptedWorkPending));
    }
    let commands = match material::commands(database, snapshot, session).await? {
        Ok(rows) => rows,
        Err(hold) => return Ok(Err(hold)),
    };
    let outbox = match material::outbox(database, &snapshot.allocation, session).await? {
        Ok(rows) => rows,
        Err(hold) => return Ok(Err(hold)),
    };
    let mut logs = Vec::new();
    crate::store::agent_log_integrity::scan_collection(
        &majority_collection(database, "agent_log"),
        &agent.id,
        agent.conversation.last_seq,
        session,
        false,
        |entry| {
            logs.push(
                digest(&entry).map_err(|_| {
                    layer2_core::sandbox::store::SandboxStoreError::SequenceConflict
                })?,
            );
            Ok(())
        },
    )
    .await
    .map_err(|error| match error {
        crate::store::agent_log_integrity::TransactionLogError::Domain(_) => {
            MutationError::Core(AllocationError::Corrupt)
        }
        crate::store::agent_log_integrity::TransactionLogError::Driver(error) => {
            MutationError::Mongo(error)
        }
    })?;
    let mut ancillary = Vec::new();
    // These records remain permanently retained. Their exact bounded snapshot
    // supplements the source/idempotency ledger and archive retention inventory.
    for collection in [
        "agent_artifacts",
        "command_authorization_audits",
        "checkpoints",
    ] {
        let filter = doc! {"$or": [
            {"agent_id": agent.id.as_str()}, {"record.agent_id": agent.id.as_str()},
            {"sandbox_id": snapshot.sandbox.id.as_str()}, {"record.sandbox_id": snapshot.sandbox.id.as_str()},
            {"command_id": {"$in": commands.iter().map(|command| command.id.as_str()).collect::<Vec<_>>()}}
        ]};
        let rows = rows(database, collection, filter, session).await?;
        if rows.len() > MAX_ROWS as usize {
            return Ok(Err(HistoryHold::InventoryBoundExceeded));
        }
        for row in rows {
            ancillary.push(project(collection, &row)?);
        }
    }
    Ok(Ok(MaterialInventory {
        agent_conversation_sha256: digest(&agent.conversation)?,
        log_count: agent.conversation.last_seq,
        log_sha256: digest(&logs)?,
        commands,
        outbox,
        ancillary,
    }))
}

pub(super) async fn rows(
    database: &Database,
    collection: &str,
    filter: Document,
    session: &mut ClientSession,
) -> Result<Vec<Document>, MutationError> {
    let mut cursor = majority_collection::<Document>(database, collection)
        .find(filter)
        .sort(doc! {"_id": 1_i32})
        .limit(MAX_ROWS + 1)
        .session(&mut *session)
        .await?;
    let mut rows = Vec::new();
    let mut bytes = 0_usize;
    while let Some(row) = cursor.next(&mut *session).await.transpose()? {
        bytes = bytes
            .checked_add(
                mongodb::bson::serialize_to_vec(&row)
                    .map_err(|_| AllocationError::Corrupt)?
                    .len(),
            )
            .filter(|bytes| *bytes <= 16 * 1024 * 1024)
            .ok_or(AllocationError::AuthorityUnavailable)?;
        rows.push(row);
    }
    Ok(rows)
}
pub(super) fn project(collection: &str, row: &Document) -> Result<InventoryRow, AllocationError> {
    Ok(InventoryRow {
        collection: collection.into(),
        id: row
            .get_str("_id")
            .map_err(|_| AllocationError::Corrupt)?
            .into(),
        sha256: digest(row.get("record").ok_or(AllocationError::Corrupt)?)?,
    })
}

pub(super) async fn require_reference(
    database: &Database,
    allocation: &AllocationRecord,
    kind: ReferenceKind,
    row: &Document,
    session: &mut ClientSession,
) -> Result<bool, MutationError> {
    let expected = Reference::capture(allocation, kind, row)?;
    let Some(reference) = majority_collection::<Document>(database, REFERENCES)
        .find_one(doc! {"_id": expected.id()?})
        .session(session)
        .await?
    else {
        return Ok(false);
    };
    if read_reference(&reference)? != expected {
        return Err(AllocationError::IdentityMismatch.into());
    }
    Ok(true)
}
