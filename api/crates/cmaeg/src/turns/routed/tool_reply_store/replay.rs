//! Replaying a retained receipt is read-only and needs no new send authority.
use super::*;

impl ReplyStore {
    pub(in crate::turns::routed) async fn automatic_receipt(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        operation: &str,
    ) -> Result<Option<StoredReceipt>, ExecutionError> {
        let record = self.load(plan, target).await?;
        Ok(record
            .automatic
            .filter(|attempt| attempt.operation == operation)
            .and_then(|attempt| attempt.receipt))
    }

    pub(in crate::turns::routed) async fn recover_answer(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        operation: &str,
        content_hash: &str,
    ) -> Result<Option<StoredReceipt>, ExecutionError> {
        let record = self.load(plan, target).await?;
        let ops = OperationStore(self.0.clone());
        if let Some(receipt) = ops
            .retained_receipt(&record, operation, content_hash)
            .await?
        {
            return Ok(Some(receipt));
        }
        if !record.operation_keys.contains(operation) {
            return Ok(None);
        }
        let attempt = record.keyed_attempt(operation)?;
        if attempt.content_hash.as_deref() != Some(content_hash) {
            return Err(ExecutionError::IdempotencyConflict);
        }
        ops.recover_answer(&self.1, &record, attempt).await
    }

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
                let attempt = record.keyed_attempt(operation)?;
                if attempt.content_hash.as_deref() != Some(content_hash) {
                    return Err(ExecutionError::IdempotencyConflict);
                }
                return Ok(Some(if attempt.until > (self.0.clock)() {
                    Admission::Pending
                } else {
                    Admission::Unknown
                }));
            }
        }
        if let Some(receipt) = self
            .exact_confirmation(&record, operation, content_hash.is_some())
            .await?
        {
            return Ok(Some(Admission::Replay(receipt)));
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
    // The OFF unkeyed writer stores the exact operation receipt only in tool.
    // Enabling keyed custody cannot make that retained confirmation disappear.
    // A matching operation is essential: confirmed_tool alone proves no intent.
    pub(super) async fn exact_confirmation(
        &self,
        record: &Record,
        operation: &str,
        legacy_only: bool,
    ) -> Result<Option<IntentReceipt>, ExecutionError> {
        let Some(attempt) = record.tool.as_ref().filter(|attempt| {
            attempt.operation == operation && (!legacy_only || attempt.content_hash.is_none())
        }) else {
            return self.legacy_confirmation(record, operation).await;
        };
        let Some(receipt) = attempt
            .receipt
            .as_ref()
            .filter(|receipt| matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }))
        else {
            return self.legacy_confirmation(record, operation).await;
        };
        self.0
            .receipts(operation, &record.owner, record.retained_until)
            .load(receipt)
            .await
            .map(Some)
            .map_err(|_| ExecutionError::Unavailable)
    }
}
