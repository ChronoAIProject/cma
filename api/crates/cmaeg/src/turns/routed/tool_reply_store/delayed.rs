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
        self.admit_keyed_inner(
            plan,
            target,
            (operation, content_hash, None),
            Kind::Tool,
            true,
            true,
        )
        .await
    }
    pub async fn admit_delayed_answer(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        operation: String,
        content_hash: String,
        answer: &AcceptedAnswerIdentity,
    ) -> Result<Admission, ExecutionError> {
        self.0.require_reply_open(plan, target).await?;
        self.admit_keyed_inner(
            plan,
            target,
            (operation, content_hash, Some(answer)),
            Kind::Tool,
            true,
            true,
        )
        .await
    }

    pub async fn refuse_delayed_answer(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        operation: &str,
        content_hash: &str,
        answer: &AcceptedAnswerIdentity,
        reason: &str,
    ) -> Result<IntentReceipt, ExecutionError> {
        let record = self.load(plan, target).await?;
        let operations = OperationStore(self.0.clone());
        if let Some(receipt) = operations.lookup(&record, operation, content_hash).await? {
            return Ok(receipt);
        }
        let receipt = if record.operation_keys.contains(operation) {
            let attempt = record.keyed_attempt(operation)?;
            if attempt.content_hash.as_deref() != Some(content_hash)
                || attempt.answer.as_ref() != Some(answer)
            {
                return Err(ExecutionError::IdempotencyConflict);
            }
            operations
                .recover_answer(&self.1, &record, attempt)
                .await?
                .ok_or(ExecutionError::Corrupt)?
        } else {
            let refusal = Attempt {
                claim: uuid::Uuid::new_v4().to_string(),
                operation: operation.into(),
                content_hash: Some(content_hash.into()),
                until: (self.0.clock)(),
                receipt: None,
                answer: Some(answer.clone()),
            };
            operations
                .close_unprepared(&self.1, &record, &refusal, reason)
                .await?
        };
        self.0
            .receipts(operation, &record.owner, record.retained_until)
            .load(&receipt)
            .await
            .map_err(|_| ExecutionError::Unavailable)
    }
    pub async fn recover_delayed(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        operation: &str,
        content_hash: &str,
        retained: Option<&StoredReceipt>,
    ) -> Result<IntentReceipt, ExecutionError> {
        let mut record = self.load(plan, target).await?;
        let replay = self
            .replay(plan, target, operation, Some(content_hash))
            .await?;
        if let Some(Admission::Replay(receipt)) = replay {
            return Ok(receipt);
        }
        if let Some(attempt) = [&record.tool, &record.automatic]
            .into_iter()
            .flatten()
            .find(|attempt| attempt.operation == operation)
            .filter(|attempt| attempt.content_hash.as_deref() == Some(content_hash))
        {
            if let Some(receipt) = OperationStore(self.0.clone())
                .recover_answer(&self.1, &record, attempt)
                .await?
            {
                return self
                    .0
                    .receipts(operation, &record.owner, record.retained_until)
                    .load(&receipt)
                    .await
                    .map_err(|_| ExecutionError::Unavailable);
            }
        }
        let stored = retained.cloned().unwrap_or_else(|| {
            crate::delivery::durable::receipt_for(DeliveryOutcome::Unknown {
                reason: "delayed_reply_retained_unknown".into(),
            })
            .into()
        });
        let receipt = self
            .0
            .receipts(operation, &record.owner, record.retained_until)
            .load(&stored)
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        // Sending or retained Unknown proves this occurrence may have had IO. Its fenced owner
        // makes that uncertainty durable without borrowing the old worker claim.
        if let Some(attempt) = record
            .tool
            .as_mut()
            .filter(|a| a.operation == operation && a.content_hash.as_deref() == Some(content_hash))
        {
            if attempt.receipt.is_none() {
                attempt.receipt = Some(stored.clone());
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
                answer: None,
                until: (self.0.clock)(),
                receipt: Some(stored),
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
