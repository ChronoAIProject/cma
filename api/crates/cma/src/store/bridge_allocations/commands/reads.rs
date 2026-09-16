use super::*;

impl MongoBridgeAllocationStore {
    pub(super) async fn command_recover_in(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
        digest: &str,
        session: &mut ClientSession,
    ) -> Result<Option<PartitionCommandReceipt>, Error> {
        let receipts =
            majority_collection::<Document>(&self.command_database(), journal::COLLECTION);
        let Some(row) = receipts.find_one(doc! {"_id":id}).session(session).await? else {
            return Ok(None);
        };
        let journal = journal::decode(&row)?;
        if journal.receipt.id != id
            || journal.receipt.bridge_id != manager.record().id
            || &journal.receipt.actor != manager.actor()
        {
            return Err(PartitionCommandError::Corrupt.into());
        }
        if journal.receipt.request_sha256 != digest {
            return Err(PartitionCommandError::Conflict.into());
        }
        Ok(Some(journal.receipt))
    }
    pub(super) async fn command_snapshot_in(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
        session: &mut ClientSession,
    ) -> Result<PartitionCommandSnapshot, Error> {
        manager.require_current()?;
        let row = self
            .records
            .find_one(doc! {"_id":id,"bridge_id":manager.record().id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(PartitionCommandError::NotFound)?;
        let allocation = decode_record(&row)?;
        let head = self
            .heads
            .find_one(doc! {"_id":allocation.identity.head_id()?})
            .session(&mut *session)
            .await?
            .ok_or(PartitionCommandError::Corrupt)?;
        if decode_head(&head, &allocation.identity)? != allocation.id {
            return Err(PartitionCommandError::Conflict.into());
        }
        // A fabricated Retired current head cannot authorize explicit rebinding.
        if allocation.state == layer2_core::bridge::allocation::AllocationState::Retired {
            self.require_detached_head(&allocation, session).await?;
        }
        let source =
            sources::original(&self.command_database(), &row, &allocation, session).await?;
        let snapshot = PartitionCommandSnapshot { allocation, source };
        validate_snapshot(manager, &snapshot)?;
        // The exact head and record also conflict with concurrent resets and admissions.
        for (collection, filter) in [
            (
                &self.heads,
                doc! {"_id":snapshot.allocation.identity.head_id()?,"allocation_id":&snapshot.allocation.id},
            ),
            (
                &self.records,
                doc! {"_id":&snapshot.allocation.id,"revision":snapshot.allocation.revision as i64},
            ),
        ] {
            if collection
                .update_one(filter, doc! {"$inc":{"partition_command_fence":1_i64}})
                .session(&mut *session)
                .await?
                .matched_count
                != 1
            {
                return Err(PartitionCommandError::Conflict.into());
            }
        }
        Ok(snapshot)
    }
}
