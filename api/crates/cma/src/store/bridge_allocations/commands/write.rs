use super::*;

impl MongoBridgeAllocationStore {
    pub(super) async fn command_apply_in(
        &self,
        write: &PartitionCommandWrite,
        reset: Option<&AllocationReservation>,
        session: &mut ClientSession,
    ) -> Result<PartitionCommandReceipt, Error> {
        if let Some(receipt) = self
            .command_recover_in(
                write.management(),
                write.receipt_id(),
                write.request_sha256(),
                session,
            )
            .await?
        {
            return Ok(receipt);
        }
        write.require_current()?;
        let snapshot = self
            .command_snapshot_in(write.management(), &write.request().allocation_id, session)
            .await?;
        if snapshot.allocation != write.snapshot().allocation
            || snapshot.source != write.snapshot().source
        {
            return Err(PartitionCommandError::Conflict.into());
        }
        let now = reset
            .map(|r| r.record().created_at_ms)
            .unwrap_or_else(|| mongodb::bson::DateTime::now().timestamp_millis());
        let retired = write.retired(now)?;
        let successor = match (write.request().action, reset) {
            (PartitionCommandAction::Reset, Some(reservation)) => {
                if reservation.previous() != Some(&snapshot.allocation)
                    || reservation.reset_management().is_none_or(|m| {
                        m.record() != write.management().record()
                            || m.actor() != write.management().actor()
                    })
                {
                    return Err(PartitionCommandError::Corrupt.into());
                }
                Some(
                    self.transaction(&Write::Reserve(reservation), session)
                        .await?,
                )
            }
            (PartitionCommandAction::Detach, None) => {
                // Usage detachment does not revoke grants or change historical attribution.
                if let Some(invokes) = &self.invokes {
                    invokes.detach_allocation_usage(write, session).await?;
                } else {
                    let row = self
                        .records
                        .find_one(doc! {"_id":&snapshot.allocation.id})
                        .session(&mut *session)
                        .await?
                        .ok_or(PartitionCommandError::Corrupt)?;
                    if row.contains_key("channel_usage_binding") {
                        return Err(PartitionCommandError::Unavailable.into());
                    }
                }
                self.replace(&snapshot.allocation, &retired, session)
                    .await?;
                None
            }
            _ => return Err(PartitionCommandError::Corrupt.into()),
        };
        let receipt = write.receipt(&retired, successor.as_ref())?;
        let journal = journal::Journal {
            receipt: receipt.clone(),
            previous: snapshot.allocation,
            result: retired,
            successor,
            source: snapshot.source,
        };
        let marker = doc! {"schema_version":1_i32,"receipt_id":&receipt.id};
        let mut fields = doc! {"partition_command_source":marker.clone()};
        if receipt.action == PartitionCommandAction::Detach {
            fields.insert("partition_detachment", marker.clone());
        }
        if self
            .records
            .update_one(
                doc! {"_id":&receipt.allocation_id,"revision":receipt.result.revision as i64},
                doc! {"$set":fields},
            )
            .session(&mut *session)
            .await?
            .matched_count
            != 1
        {
            return Err(PartitionCommandError::Conflict.into());
        }
        if let Some(next) = &journal.successor {
            if self
                .records
                .update_one(
                    doc! {"_id":&next.id,"revision":next.revision as i64},
                    doc! {"$set":{"partition_command_source":marker}},
                )
                .session(&mut *session)
                .await?
                .matched_count
                != 1
            {
                return Err(PartitionCommandError::Conflict.into());
            }
        }
        majority_collection::<Document>(&self.command_database(), journal::COLLECTION)
            .insert_one(journal.document()?)
            .session(&mut *session)
            .await?;
        for allocation in std::iter::once(&journal.result).chain(journal.successor.iter()) {
            crate::store::channel_history::partition_command(
                &self.command_database(),
                allocation,
                &journal.document()?,
                session,
            )
            .await?;
        }
        write.require_current()?;
        Ok(receipt)
    }
}
