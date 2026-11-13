//! Replaying a retained receipt is read-only and needs no new send authority.
use super::*;

impl ReplyStore {
    pub async fn replay(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        operation: &str,
        content_hash: Option<&str>,
    ) -> Result<Option<Admission>, ExecutionError> {
        let record = self.load(plan, target).await?;
        if let Some(content_hash) = content_hash {
            if record.operation_keys.contains(operation) {
                if let Some(receipt) = OperationStore(self.0.clone())
                    .lookup(&record, operation, content_hash)
                    .await?
                {
                    return Ok(Some(Admission::Replay(receipt)));
                }
                let attempt = record
                    .tool
                    .as_ref()
                    .filter(|attempt| attempt.operation == operation)
                    .ok_or(ExecutionError::Corrupt)?;
                if attempt.content_hash.as_deref() != Some(content_hash) {
                    return Err(ExecutionError::IdempotencyConflict);
                }
                return Ok(Some(if attempt.until > (self.0.clock)() {
                    Admission::Pending
                } else {
                    Admission::Unknown
                }));
            }
        } else if let Some(attempt) = &record.tool {
            if attempt.operation == operation {
                if let Some(receipt) = &attempt.receipt {
                    if matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }) {
                        return self
                            .0
                            .receipts(operation, &record.owner, record.retained_until)
                            .load(receipt)
                            .await
                            .map(|receipt| Some(Admission::Replay(receipt)))
                            .map_err(|_| ExecutionError::Unavailable);
                    }
                }
            }
        }
        if let Some(status) = self.unsettled(&record) {
            return Ok(Some(if status == ToolReplyStatus::Pending {
                Admission::Pending
            } else {
                Admission::Unknown
            }));
        }
        Ok(record.closed.then_some(Admission::Closed))
    }
}
