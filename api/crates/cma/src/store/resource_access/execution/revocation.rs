use super::*;
use futures_util::TryStreamExt;
use layer2_core::resource_access::{ShareChange, ShareMutation};

/// Only the transaction that reread all frozen evidence can construct this.
pub(in crate::store::resource_access) struct FinalizationPermit {
    request: ShareMutation,
}
impl FinalizationPermit {
    pub(in crate::store::resource_access) fn request(&self) -> &ShareMutation {
        &self.request
    }
}

impl MongoResourceExecutionStore {
    pub(super) async fn freeze(
        &self,
        authority: &RevocationAuthority,
        session: &mut ClientSession,
    ) -> TxnResult<RevocationOperationV1> {
        let intent = authority.intent();
        let id = wire::Digest::new(
            &intent
                .mutation(intent.original_policy_revision)
                .operation_id(),
        )?;
        let snapshot =
            super::super::snapshot::read(&self.sandbox, &intent.selector.target(), session).await?;
        intent.validate_incarnation(&snapshot)?;
        if let Some(document) = self
            .collections
            .operations
            .find_one(doc! {"_id":id.as_str()})
            .session(&mut *session)
            .await?
        {
            let previous = records::read_operation(&document)?;
            if previous.intent.fingerprint != intent.fingerprint
                || previous.epoch != authority.epoch()
            {
                return Err(ExecutionError::IdempotencyConflict.into());
            }
            previous.intent.validate_incarnation(&snapshot)?;
            return Ok(previous);
        }
        authority.validate_current(&snapshot)?;
        let epoch = self.collections.require_activation(session).await?;
        if epoch != authority.epoch() {
            return Err(ExecutionError::AuthorityChanged.into());
        }
        let mut control = self.grant_in(authority.family(), epoch, session).await?;
        control.revision = control.revision.successor()?;
        control.state = GrantStateV1::Closing {
            operation: id.clone(),
        };
        let at = unix_millis_now()?;
        let operation = RevocationOperationV1 {
            protocol: 1,
            key: wire::RevocationKeyV1 {
                operation_id: id,
                family: control.key.clone(),
                watermark: control.revision,
            },
            epoch,
            intent: intent.clone(),
            frozen_members: control.members,
            erased_ordinals: Default::default(),
            frozen_at_unix_ms: at,
            deadline_unix_ms: at
                .checked_add(wire::STOP_BOUND_MS)
                .ok_or(ExecutionError::Capacity)?,
            phase: RevocationPhaseV1::Pending,
        };
        super::super::mutation::fence_lineage(&self.sandbox, &snapshot, session).await?;
        let write_keys = self
            .writes_erasure_keys(Some(&control.key), session)
            .await?;
        let grant_keys = write_keys
            .then(|| {
                erasure::keys(
                    intent.sandbox_id.as_str(),
                    &intent.owner,
                    None,
                    Some(&control.key.share_id),
                )
            })
            .transpose()?;
        self.write_grant(&control, grant_keys, session).await?;
        // The immutable generation plus member ordinals is the durable outbox.
        // No notification, expired lease or lost commit reply can omit a member.
        let mut document = records::operation_document(&operation)?;
        if write_keys {
            document.insert(
                records::ERASURE_KEYS,
                erasure::keys(
                    intent.sandbox_id.as_str(),
                    &intent.owner,
                    intent.actor.ordinary_subject(),
                    Some(&operation.key.family.share_id),
                )?,
            );
        }
        self.collections
            .operations
            .insert_one(document)
            .session(session)
            .await?;
        tracing::debug!(
            members = operation.frozen_members,
            "resource revocation cohort prepared for commit"
        );
        Ok(operation)
    }

    pub(super) async fn persist_barrier(
        &self,
        receipt: &wire::BarrierReceiptV1,
        session: &mut ClientSession,
    ) -> TxnResult<()> {
        let operation = self
            .operation_in(&receipt.request.revocation.operation_id, session)
            .await?;
        let mut member = self
            .member_in(&receipt.request.registration, session)
            .await?;
        let request = member.barrier_request(&operation)?;
        member.observe_barrier(&request, receipt)?;
        self.save_member(&member, session).await
    }

    pub(super) async fn persist_completion(
        &self,
        completion: &MemberCompletionV1,
        session: &mut ClientSession,
    ) -> TxnResult<()> {
        let barrier = match completion {
            MemberCompletionV1::Execution { receipt } => &receipt.barrier,
            MemberCompletionV1::Release { receipt } => &receipt.barrier,
        };
        let operation = self
            .operation_in(&barrier.request.revocation.operation_id, session)
            .await?;
        let mut member = self
            .member_in(&barrier.request.registration, session)
            .await?;
        let request = member.barrier_request(&operation)?;
        // Barrier evidence is a separate durable observation; a StopReceipt
        // arriving first must be retried after its barrier was recorded.
        barrier.validate_for(&request)?;
        member.observe_completion(completion)?;
        self.save_member(&member, session).await
    }

    pub(super) async fn finalize(
        &self,
        id: &wire::Digest,
        session: &mut ClientSession,
    ) -> TxnResult<RevocationOperationV1> {
        let mut operation = self.operation_in(id, session).await?;
        let target = operation.intent.selector.target();
        let snapshot = super::super::snapshot::read(&self.sandbox, &target, session).await?;
        operation.intent.validate_incarnation(&snapshot)?;
        if matches!(operation.phase, RevocationPhaseV1::Effective { .. }) {
            return Ok(operation);
        }
        let document = self
            .collections
            .grants
            .find_one(doc! {"_id":operation.key.family.origin.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        let mut control = records::read_grant(&document)?;
        if control.key != operation.key.family
            || control.epoch != operation.epoch
            || control.members != operation.frozen_members
            || control.state
                != (GrantStateV1::Closing {
                    operation: id.clone(),
                })
        {
            return Err(ExecutionError::EvidenceMismatch.into());
        }
        let mut cursor = self
            .collections
            .members
            .find(family_filter(&operation.key.family))
            .sort(doc! {"ordinal":1})
            .limit(i64::from(MAX_FAMILY_MEMBERS) + 1)
            .session(&mut *session)
            .await?;
        let mut members = Vec::with_capacity(operation.frozen_members as usize);
        while let Some(document) = cursor.next(&mut *session).await.transpose()? {
            members.push(records::read_member(&document)?);
        }
        operation.validate_cohort(&members)?;
        let request = operation.intent.mutation(snapshot.fence().policy_revision);
        let permit = FinalizationPermit { request };
        let receipt =
            super::super::mutation::apply_execution_finalization(&self.sandbox, &permit, session)
                .await?;
        operation.phase = RevocationPhaseV1::Effective {
            receipt: Box::new(receipt),
        };
        control.revision = control.revision.successor()?;
        match operation.intent.change {
            ShareChange::SetMode { .. } => {
                control.key.generation = control.key.generation.successor()?;
                control.members = 0;
                control.state = GrantStateV1::Open;
            }
            ShareChange::Revoke { .. } => {
                control.state = GrantStateV1::Closed {
                    operation: id.clone(),
                }
            }
            _ => return Err(ExecutionError::Corrupt.into()),
        }
        self.write_grant(&control, None, session).await?;
        let result = self
            .collections
            .operations
            .update_one(
                doc! {"_id":id.as_str(),"phase":"pending"},
                records::update_record(records::operation_document(&operation)?),
            )
            .session(session)
            .await?;
        if result.matched_count != 1 {
            return Err(ExecutionError::Corrupt.into());
        }
        tracing::debug!(
            members = operation.frozen_members,
            "resource revocation completion prepared for commit"
        );
        Ok(operation)
    }

    pub(super) async fn pending_page(
        &self,
        after: Option<&wire::Digest>,
        limit: u32,
    ) -> ExecutionResult<Vec<RevocationOperationV1>> {
        page_limit(limit)?;
        let mut filter = doc! {"phase":"pending"};
        if let Some(after) = after {
            filter.insert("_id", doc! {"$gt":after.as_str()});
        }
        let documents: Vec<_> = self
            .collections
            .operations
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(i64::from(limit))
            .await
            .map_err(|_| ExecutionError::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        documents.iter().map(records::read_operation).collect()
    }

    pub(super) async fn member_page(
        &self,
        id: &wire::Digest,
        after: u32,
        limit: u32,
    ) -> ExecutionResult<Vec<ExecutionRecordV1>> {
        page_limit(limit)?;
        let operation = self
            .revocation(id)
            .await?
            .ok_or(ExecutionError::AuthorityChanged)?;
        if after > operation.frozen_members {
            return Err(ExecutionError::EvidenceMismatch);
        }
        let mut filter = family_filter(&operation.key.family);
        filter.insert(
            "ordinal",
            doc! {"$gt":i64::from(after),"$lte":i64::from(operation.frozen_members)},
        );
        let documents: Vec<_> = self
            .collections
            .members
            .find(filter)
            .sort(doc! {"ordinal":1})
            .limit(i64::from(limit))
            .await
            .map_err(|_| ExecutionError::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        let members = documents
            .iter()
            .map(records::read_member)
            .collect::<ExecutionResult<Vec<_>>>()?;
        let mut previous = after;
        for member in &members {
            if operation.next_retained_ordinal(previous) != Some(member.ordinal) {
                return Err(ExecutionError::Corrupt);
            }
            member.barrier_request(&operation)?;
            previous = member.ordinal;
        }
        if members.len() < limit as usize && operation.next_retained_ordinal(previous).is_some() {
            return Err(ExecutionError::Corrupt);
        }
        Ok(members)
    }
}

pub(super) fn family_filter(family: &wire::GrantFamilyKey) -> Document {
    doc! {"family_origin":family.origin.as_str(),"generation":family.generation.get() as i64}
}
fn page_limit(limit: u32) -> ExecutionResult<()> {
    if limit == 0 || limit > MAX_PAGE_MEMBERS {
        return Err(ExecutionError::Capacity);
    }
    Ok(())
}
