//! Validate retained reply authority and reconcile keyed operation receipts.
use super::*;

impl ReplyStore {
    pub(super) fn unsettled(&self, record: &Record) -> Option<ToolReplyStatus> {
        self.unsettled_at(record, (self.0.clock)())
    }
    pub(super) fn unsettled_at(&self, record: &Record, now: i64) -> Option<ToolReplyStatus> {
        let mut result = None;
        for attempt in [&record.tool, &record.automatic].into_iter().flatten() {
            match &attempt.receipt {
                None if attempt.until > now => result = Some(ToolReplyStatus::Pending),
                None => return Some(ToolReplyStatus::Unknown),
                Some(receipt) if receipt.requires_reconciliation() => {
                    return Some(ToolReplyStatus::Unknown);
                }
                _ => {}
            }
        }
        result
    }
    pub(in crate::turns::routed) async fn load(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Record, ExecutionError> {
        if plan.retained_until <= (self.0.clock)() {
            return Err(ExecutionError::Expired);
        }
        let id = storage::identifier(&("ordinary-route-reply-guard-v1", &target.id));
        let expected = Record {
            kind: "ordinary_route_reply_guard_v1".into(),
            id: id.clone(),
            target: target.id.clone(),
            root: plan.admission_id.clone(),
            owner: plan.identity.owner.clone(),
            request_hash: plan.request_hash.clone(),
            retained_until: plan.retained_until,
            confirmed_tool: false,
            operation_keys: BTreeSet::new(),
            legacy_confirmation: None,
            closed: false,
            tool: None,
            automatic: None,
            version: 0,
        };
        let Some((mut record, version)) = self.0.read::<Record>(&id).await? else {
            return Ok(expected);
        };
        if record.kind != expected.kind
            || record.id != id
            || record.target != expected.target
            || record.root != expected.root
            || record.owner != expected.owner
            || record.request_hash != expected.request_hash
            || record.retained_until != expected.retained_until
            || storage::bytes(&record)?.len() > 40 * 1024
            || record.operation_keys.len() + usize::from(record.legacy_confirmation.is_some()) > 64
            || record
                .legacy_confirmation
                .as_ref()
                .is_some_and(|id| !valid_hash(id) || record.operation_keys.contains(id))
            || record.operation_keys.iter().any(|key| !valid_hash(key))
            || [&record.tool, &record.automatic]
                .into_iter()
                .flatten()
                .any(|attempt| {
                    uuid::Uuid::parse_str(&attempt.claim).is_err()
                        || !valid_hash(&attempt.operation)
                        || attempt.content_hash.as_ref().is_some_and(|hash| {
                            !valid_hash(hash) || !record.operation_keys.contains(&attempt.operation)
                        })
                        || attempt.answer.as_ref().is_some_and(|answer| {
                            attempt.content_hash.is_none()
                                || answer.operation_id(&record.target).ok().as_deref()
                                    != Some(attempt.operation.as_str())
                        })
                })
        {
            return Err(ExecutionError::Corrupt);
        }
        record.version = version;
        if let Some(attempt) = &record.automatic {
            if attempt.content_hash.is_some() && attempt.receipt.is_none() {
                let operation = OperationStore(self.0.clone());
                let receipt = operation.reconcile(&record, attempt).await?;
                record.automatic.as_mut().unwrap().receipt = receipt;
            }
        }
        if let Some(attempt) = &record.tool {
            if attempt.content_hash.is_some() && attempt.receipt.is_none() {
                if let Some(receipt) = OperationStore(self.0.clone())
                    .reconcile(&record, attempt)
                    .await?
                {
                    record.confirmed_tool |=
                        matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. });
                    record.tool.as_mut().unwrap().receipt = Some(receipt);
                    // This is a read-only projection of an already durable
                    // receipt. The next authorized admission/close persists it
                    // with its captured partition token; a read never gains one.
                }
            }
        }
        Ok(record)
    }
}
