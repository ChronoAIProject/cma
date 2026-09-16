use layer2_core::bridge::allocation::{
    AllocationError, AllocationRecord, AllocationReservation, AllocationState,
};
use mongodb::{bson::doc, ClientSession};

use super::{
    records::{decode_head, decode_record, head_document, record_document},
    MongoBridgeAllocationStore, MutationError, Write,
};

impl MongoBridgeAllocationStore {
    pub(super) async fn transaction(
        &self,
        write: &Write<'_>,
        session: &mut ClientSession,
    ) -> Result<AllocationRecord, MutationError> {
        write.require_time()?;
        let reservation = matches!(write, Write::Reserve(_));
        if reservation {
            // Current authority precedes inventory; cleanup exclusion follows
            // actual capacity checks, before any transactional write commits.
            self.fence_authority(write.authority(), session).await?;
        } else {
            self.fence(write.authority(), session).await?;
        }
        let result = match write {
            Write::Reserve(write) => self.reserve_transaction(write, &mut *session).await?,
            Write::Mutate(write) => {
                let current = self
                    .records
                    .find_one(doc! {"_id":&write.record().id})
                    .session(&mut *session)
                    .await?
                    .ok_or(AllocationError::Conflict)?;
                let current = decode_record(&current)?;
                if &current == write.record() {
                    current
                } else {
                    if &current != write.previous() {
                        return Err(AllocationError::Conflict.into());
                    }
                    self.replace(&current, write.record(), &mut *session)
                        .await?;
                    write.record().clone()
                }
            }
        };
        if reservation {
            self.fence_reuse_cleanup(write.authority(), session).await?;
        }
        write.require_time()?;
        Ok(result)
    }

    #[cfg(test)]
    pub(in crate::store) async fn reserve_in_session(
        &self,
        write: &AllocationReservation,
        session: &mut ClientSession,
    ) -> Result<AllocationRecord, MutationError> {
        self.transaction(&Write::Reserve(write), session).await
    }

    async fn reserve_transaction(
        &self,
        write: &AllocationReservation,
        session: &mut ClientSession,
    ) -> Result<AllocationRecord, MutationError> {
        let record = write.record();
        let head = self
            .heads
            .find_one(doc! {"_id":record.identity.head_id()?})
            .session(&mut *session)
            .await?;
        if let Some(head) = head {
            let id = decode_head(&head, &record.identity)?;
            let current = self
                .records
                .find_one(doc! {"_id":id})
                .session(&mut *session)
                .await?
                .ok_or(AllocationError::Corrupt)?;
            let has_usage = current.contains_key("channel_usage_binding");
            let current = decode_record(&current)?;
            if let Some(previous) = write.previous() {
                if current.id == record.id {
                    // The reset already committed. Later state progress is
                    // retained; this retry cannot create generation N+2.
                    if current.predecessor != record.predecessor
                        || current.pin != record.pin
                        || current.config != record.config
                        || current.placement != record.placement
                        || current.owner != record.owner
                        || current.operation_id != record.operation_id
                    {
                        return Err(AllocationError::Conflict.into());
                    }
                    return Ok(current);
                }
                if &current != previous {
                    return Err(AllocationError::Conflict.into());
                }
                if has_usage && self.invokes.is_none() {
                    return Err(AllocationError::AuthorityUnavailable.into());
                }
                self.require_capacity(write, false, &mut *session).await?;
                if let Some(invokes) = &self.invokes {
                    invokes
                        .reset_allocation_usage(write, session)
                        .await
                        .map_err(|error| match error {
                            super::super::bridge_invokes::MutationError::Mongo(error) => {
                                MutationError::Mongo(error)
                            }
                            super::super::bridge_invokes::MutationError::Invoke(error) => {
                                MutationError::Core(match error {
                                    layer2_core::bridge::invoke::BridgeInvokeError::Conflict => {
                                        AllocationError::Conflict
                                    }
                                    layer2_core::bridge::invoke::BridgeInvokeError::Corrupt => {
                                        AllocationError::Corrupt
                                    }
                                    _ => AllocationError::AuthorityUnavailable,
                                })
                            }
                        })?;
                }
                let mut retired = current.clone();
                retired.state = AllocationState::Retired;
                retired.lease = None;
                retired.revision += 1;
                retired.updated_at_ms = record.created_at_ms;
                self.replace(&current, &retired, &mut *session).await?;
                self.records
                    .insert_one(crate::store::channel_history::new_document(
                        record,
                        record_document(record)?,
                    )?)
                    .session(&mut *session)
                    .await?;
                crate::store::channel_history::seed(
                    &self.bridges.client.database(&self.records.namespace().db),
                    record,
                    session,
                )
                .await?;
                let changed = self
                    .heads
                    .replace_one(
                        doc! {"_id":record.identity.head_id()?,"allocation_id":&current.id},
                        head_document(record)?,
                    )
                    .session(&mut *session)
                    .await?;
                if changed.matched_count != 1 {
                    return Err(AllocationError::Conflict.into());
                }
                return Ok(record.clone());
            }
            current.require_same_partition(record)?;
            return Ok(current);
        }
        if write.previous().is_some() {
            return Err(AllocationError::Conflict.into());
        }
        self.require_capacity(write, true, &mut *session).await?;
        self.records
            .insert_one(crate::store::channel_history::new_document(
                record,
                record_document(record)?,
            )?)
            .session(&mut *session)
            .await?;
        crate::store::channel_history::seed(
            &self.bridges.client.database(&self.records.namespace().db),
            record,
            session,
        )
        .await?;
        self.heads
            .insert_one(head_document(record)?)
            .session(&mut *session)
            .await?;
        Ok(record.clone())
    }

    async fn require_capacity(
        &self,
        write: &AllocationReservation,
        new_head: bool,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let record = write.record();
        if matches!(
            record.placement,
            layer2_core::bridge::allocation::AllocationPlacement::ReuseParent { .. }
        ) {
            self.require_reuse_capacity(record, session).await?;
        }
        if new_head
            && self
                .heads
                .count_documents(doc! {"bridge_id":record.identity.bridge_id.as_str()})
                .session(&mut *session)
                .await?
                >= u64::from(record.config.limits.max_partitions)
        {
            return Err(AllocationError::Capacity.into());
        }
        // Retired resource trees still consume capacity until an explicit,
        // independently authorized deletion/reconciliation releases them.
        if record.placement.creates_sandbox()
            && self
                .records
                .count_documents(
                    doc! {"bridge_id":record.identity.bridge_id.as_str(),"creates_sandbox":true},
                )
                .session(&mut *session)
                .await?
                >= u64::from(record.config.limits.max_sandboxes)
        {
            return Err(AllocationError::Capacity.into());
        }
        Ok(())
    }

    pub(super) async fn replace(
        &self,
        previous: &AllocationRecord,
        next: &AllocationRecord,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let mut fields = record_document(next)?;
        fields.remove("_id");
        let changed = self.records.update_one(
            doc! {"_id":&previous.id,"revision":previous.revision as i64,"lease_fence":previous.lease_fence as i64},
            doc! {"$set": fields},
        ).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
        Ok(())
    }
}
