use super::*;
use layer2_core::bridge::{
    allocation::AllocationState,
    ingress::{InboxAdmission, StoredInbox},
    partition::PartitionSourcePin,
};
use serde::{Deserialize, Serialize};

pub(super) const COLLECTION: &str = "bridge_partition_command_receipts";

/// Private source evidence is retained with the same permanent command receipt.
/// Only `receipt` is returned through public management.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    pub receipt: PartitionCommandReceipt,
    pub previous: AllocationRecord,
    pub result: AllocationRecord,
    pub successor: Option<AllocationRecord>,
    pub source: StoredInbox,
}
impl Journal {
    pub fn validate(&self) -> Result<(), PartitionCommandError> {
        self.receipt.validate()?;
        self.previous.validate()?;
        self.result.validate()?;
        let mut expected = self.previous.clone();
        expected.state = AllocationState::Retired;
        expected.lease = None;
        expected.revision = expected
            .revision
            .checked_add(1)
            .ok_or(PartitionCommandError::Corrupt)?;
        expected.updated_at_ms = self.receipt.created_at_ms;
        if self.result != expected
            || self.receipt.result != (&self.result).into()
            || self.previous.identity.bridge_id != self.receipt.bridge_id
            || self.previous.id != self.receipt.allocation_id
            || self.previous.revision != self.receipt.previous_revision
            || self.previous.updated_at_ms > self.receipt.created_at_ms
            || self
                .previous
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at_ms > self.receipt.created_at_ms)
            || (self.previous.effect_started && self.previous.receipt.is_none())
            || self.source.owner != self.previous.owner
            || self.source.admission != InboxAdmission::Accepted
            || PartitionSourcePin::for_inbox(&self.source)
                .map_err(|_| PartitionCommandError::Corrupt)?
                != self.previous.first_source
        {
            return Err(PartitionCommandError::Corrupt);
        }
        match (&self.successor, self.receipt.action) {
            (None, PartitionCommandAction::Detach)
                if self.previous.state != AllocationState::Retired => {}
            (Some(next), PartitionCommandAction::Reset) => {
                next.validate()?;
                if self.receipt.successor.as_ref() != Some(&next.into())
                    || next.owner != self.previous.owner
                    || next.pin != self.previous.pin
                    || next.first_source != self.previous.first_source
                    || next.config != self.previous.config
                    || next.identity.key != self.previous.identity.key
                    || next.created_at_ms != self.receipt.created_at_ms
                    || next.predecessor.as_ref().is_none_or(|p| {
                        p.id != self.previous.id
                            || p.generation != self.previous.identity.generation
                            || p.reset_by != self.receipt.actor
                    })
                {
                    return Err(PartitionCommandError::Corrupt);
                }
            }
            _ => return Err(PartitionCommandError::Corrupt),
        }
        Ok(())
    }
    pub fn document(&self) -> Result<Document, PartitionCommandError> {
        self.validate()?;
        Ok(
            doc! {"_id":&self.receipt.id,"document_schema_version":1_i32,
            "bridge_id":self.receipt.bridge_id.as_str(),"request_sha256":&self.receipt.request_sha256,
            "journal_sha256":command_hash(self)?,
            "record":mongodb::bson::serialize_to_bson(self).map_err(|_| PartitionCommandError::Corrupt)?},
        )
    }
}
pub(super) fn decode(row: &Document) -> Result<Journal, PartitionCommandError> {
    let journal: Journal = mongodb::bson::deserialize_from_bson(
        row.get("record")
            .cloned()
            .ok_or(PartitionCommandError::Corrupt)?,
    )
    .map_err(|_| PartitionCommandError::Corrupt)?;
    journal.validate()?;
    if row.get_i32("document_schema_version").ok() != Some(1)
        || row.get_str("_id").ok() != Some(&journal.receipt.id)
        || row.get_str("bridge_id").ok() != Some(journal.receipt.bridge_id.as_str())
        || row.get_str("request_sha256").ok() != Some(&journal.receipt.request_sha256)
        || row.get_str("journal_sha256").ok() != Some(command_hash(&journal)?.as_str())
    {
        return Err(PartitionCommandError::Corrupt);
    }
    Ok(journal)
}

pub(in crate::store) async fn require_detached(
    database: &Database,
    allocation: &AllocationRecord,
    write_fence: bool,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let records = majority_collection::<Document>(database, "bridge_allocations");
    let row = records
        .find_one(doc! {"_id":&allocation.id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    if decode_record(&row)? != *allocation || allocation.state != AllocationState::Retired {
        return Err(AllocationError::Conflict.into());
    }
    let marker = row
        .get_document("partition_detachment")
        .map_err(|_| AllocationError::Conflict)?;
    if marker.len() != 2 || marker.get_i32("schema_version").ok() != Some(1) {
        return Err(AllocationError::Corrupt.into());
    }
    let id = marker
        .get_str("receipt_id")
        .map_err(|_| AllocationError::Corrupt)?;
    let receipts = majority_collection::<Document>(database, COLLECTION);
    let receipt = receipts
        .find_one(doc! {"_id":id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Corrupt)?;
    let journal = decode(&receipt).map_err(|_| AllocationError::Corrupt)?;
    if journal.receipt.action != PartitionCommandAction::Detach || journal.result != *allocation {
        return Err(AllocationError::Corrupt.into());
    }
    if write_fence {
        for (collection, filter) in [(&records, row), (&receipts, receipt)] {
            if collection
                .update_one(filter, doc! {"$inc":{"retained_command_fence":1_i64}})
                .session(&mut *session)
                .await?
                .matched_count
                != 1
            {
                return Err(AllocationError::Conflict.into());
            }
        }
    }
    Ok(())
}

pub(in crate::store) async fn require_detached_then_reset(
    database: &Database,
    original: &AllocationRecord,
    current: &AllocationRecord,
    write_fence: bool,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let records = majority_collection::<Document>(database, "bridge_allocations");
    let row = records
        .find_one(doc! {"_id":&current.id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    if decode_record(&row)? != *current {
        return Err(AllocationError::Conflict.into());
    }
    let receipts = majority_collection::<Document>(database, COLLECTION);
    let mut journals = Vec::with_capacity(2);
    for field in ["partition_detachment", "partition_command_source"] {
        let marker = row
            .get_document(field)
            .map_err(|_| AllocationError::Corrupt)?;
        if marker.len() != 2 || marker.get_i32("schema_version").ok() != Some(1) {
            return Err(AllocationError::Corrupt.into());
        }
        let id = marker
            .get_str("receipt_id")
            .map_err(|_| AllocationError::Corrupt)?;
        let receipt = receipts
            .find_one(doc! {"_id":id})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::Corrupt)?;
        journals.push(decode(&receipt).map_err(|_| AllocationError::Corrupt)?);
        if write_fence
            && receipts
                .update_one(receipt, doc! {"$inc":{"retained_command_fence":1_i64}})
                .session(&mut *session)
                .await?
                .matched_count
                != 1
        {
            return Err(AllocationError::Conflict.into());
        }
    }
    let [detach, reset] = journals.as_slice() else {
        return Err(AllocationError::Corrupt.into());
    };
    if detach.receipt.action != PartitionCommandAction::Detach
        || detach.previous != *original
        || reset.receipt.action != PartitionCommandAction::Reset
        || reset.previous != detach.result
        || reset.result != *current
    {
        return Err(AllocationError::Corrupt.into());
    }
    Ok(())
}

pub(in crate::store) async fn retained_fixed_source(
    database: &Database,
    allocation: &AllocationRecord,
    write_fence: bool,
    session: &mut ClientSession,
) -> Result<AllocationRecord, MutationError> {
    let row = majority_collection::<Document>(database, "bridge_allocations")
        .find_one(doc! {"_id":&allocation.id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    if decode_record(&row)? != *allocation {
        return Err(AllocationError::Conflict.into());
    }
    let receipts = majority_collection::<Document>(database, COLLECTION);
    let mut current = allocation.clone();
    for field in ["partition_command_source", "partition_detachment"] {
        let marker = row
            .get_document(field)
            .map_err(|_| AllocationError::Corrupt)?;
        if marker.len() != 2 || marker.get_i32("schema_version").ok() != Some(1) {
            return Err(AllocationError::Corrupt.into());
        }
        let id = marker
            .get_str("receipt_id")
            .map_err(|_| AllocationError::Corrupt)?;
        let document = receipts
            .find_one(doc! {"_id":id})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::Corrupt)?;
        let journal = decode(&document).map_err(|_| AllocationError::Corrupt)?;
        if journal.result != current {
            return Err(AllocationError::Corrupt.into());
        }
        if write_fence
            && receipts
                .update_one(document, doc! {"$inc":{"retained_command_fence":1_i64}})
                .session(&mut *session)
                .await?
                .matched_count
                != 1
        {
            return Err(AllocationError::Conflict.into());
        }
        current = journal.previous;
        if current.state == AllocationState::Ready {
            return Ok(current);
        }
    }
    Err(AllocationError::Corrupt.into())
}
