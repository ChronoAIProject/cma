//! Validate retained reply authority and reconcile keyed operation receipts.
use super::*;

impl ReplyStore {
    pub(super) fn unsettled(&self, record: &Record) -> Option<ToolReplyStatus> {
        let mut result = None;
        for attempt in [&record.tool, &record.automatic].into_iter().flatten() {
            match &attempt.receipt {
                None if attempt.until > (self.0.clock)() => result = Some(ToolReplyStatus::Pending),
                None => return Some(ToolReplyStatus::Unknown),
                Some(receipt) if matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }) => {
                    return Some(ToolReplyStatus::Unknown);
                }
                _ => {}
            }
        }
        result
    }
    pub(super) async fn load(
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
            || record.operation_keys.len() > 64
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
                })
        {
            return Err(ExecutionError::Corrupt);
        }
        record.version = version;
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
