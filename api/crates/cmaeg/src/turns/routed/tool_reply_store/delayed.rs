//! Explicit delayed capabilities share ordinary attempt arbitration, including
//! normal closed guards. Closure remains sticky for ordinary immediate tools.
use super::*;
impl ReplyStore {
    pub async fn admit_delayed(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        operation: String,
        content_hash: String,
    ) -> Result<Admission, ExecutionError> {
        self.0.require_reply_open(plan, target).await?;
        self.admit_keyed_inner(plan, target, operation, content_hash, true, true)
            .await
    }
    pub async fn recover_delayed(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        operation: &str,
        content_hash: &str,
    ) -> Result<IntentReceipt, ExecutionError> {
        let mut record = self.load(plan, target).await?;
        let replay = self
            .replay(plan, target, operation, Some(content_hash))
            .await?;
        if let Some(Admission::Replay(receipt)) = replay {
            return Ok(receipt);
        }
        let receipt = crate::delivery::durable::receipt_for(DeliveryOutcome::Unknown {
            reason: "delayed_reply_retained_unknown".into(),
        });
        // Sending proves this occurrence may have had IO. Its new fenced owner
        // makes that uncertainty durable without borrowing the old worker claim.
        if let Some(attempt) = record
            .tool
            .as_mut()
            .filter(|a| a.operation == operation && a.content_hash.as_deref() == Some(content_hash))
        {
            if attempt.receipt.is_none() {
                attempt.receipt = Some(receipt.clone().into());
                self.0
                    .write_fenced(
                        &self.1,
                        &record.id,
                        &record.owner,
                        record.retained_until,
                        &record,
                        Some(record.version),
                    )
                    .await?;
            }
        } else if self.unsettled(&record).is_none() {
            self.preserve_legacy(&mut record).await?;
            record.tool = Some(Attempt {
                claim: uuid::Uuid::new_v4().to_string(),
                operation: operation.into(),
                content_hash: None,
                until: (self.0.clock)(),
                receipt: Some(receipt.clone().into()),
            });
            self.0
                .write_fenced(
                    &self.1,
                    &record.id,
                    &record.owner,
                    record.retained_until,
                    &record,
                    (record.version > 0).then_some(record.version),
                )
                .await?;
        }
        Ok(receipt)
    }
}
