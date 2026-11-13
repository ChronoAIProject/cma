//! Persist the reply outcome before releasing its admission barrier.
use super::*;

impl ReplyStore {
    pub async fn guard(&self, claim: &ReplyClaim) -> Result<(), ExecutionError> {
        if claim.until() <= (self.0.clock)() {
            return Err(ExecutionError::LostLease);
        }
        let Some((stored, version)) = self.0.read::<Record>(&claim.record.id).await? else {
            return Err(ExecutionError::LostLease);
        };
        if version != claim.record.version
            || storage::bytes(&stored)? != storage::bytes(&claim.record)?
            || claim.until() <= (self.0.clock)()
            || claim.record.retained_until <= (self.0.clock)()
        {
            return Err(ExecutionError::LostLease);
        }
        Ok(())
    }
    pub async fn finish(
        &self,
        claim: &ReplyClaim,
        receipt: &IntentReceipt,
    ) -> Result<StoredReceipt, ExecutionError> {
        self.guard(claim).await?;
        let mut next = claim.record.clone();
        let receipt =
            if receipt.message_parts.is_empty() && storage::bytes(receipt)?.len() > 16 * 1024 {
                crate::delivery::durable::receipt_for(DeliveryOutcome::Unknown {
                    reason: "reply_receipt_too_large".into(),
                })
            } else {
                receipt.clone()
            };
        let receipt = self
            .0
            .receipts(
                &claim.attempt().operation,
                &claim.record.owner,
                claim.record.retained_until,
            )
            .retain(&receipt)
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        self.guard(claim).await?;
        if claim.attempt().content_hash.is_some() {
            OperationStore(self.0.clone())
                .finish(&claim.record, claim.attempt(), &receipt)
                .await?;
        }
        if matches!(claim.kind, Kind::Tool)
            && matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. })
        {
            next.confirmed_tool = true;
        }
        let attempt = match claim.kind {
            Kind::Tool => &mut next.tool,
            Kind::Automatic => &mut next.automatic,
        };
        attempt.as_mut().unwrap().receipt = Some(receipt.clone());
        self.0
            .write(
                &next.id,
                &next.owner,
                next.retained_until,
                &next,
                Some(claim.record.version),
            )
            .await?;
        Ok(receipt)
    }
}
