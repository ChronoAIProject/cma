//! Persist the reply outcome before releasing its admission barrier.
use super::*;
use crate::turns::routed::answer_dispatch::AnswerDispatch;

impl ReplyStore {
    pub async fn renew(&self, claim: &mut ReplyClaim) -> Result<(), ExecutionError> {
        self.guard(claim).await?;
        let mut next = claim.record.clone();
        let attempt = match claim.kind {
            Kind::Tool => &mut next.tool,
            Kind::Automatic => &mut next.automatic,
        };
        attempt.as_mut().ok_or(ExecutionError::Corrupt)?.until = (self.0.clock)() + 30;
        next.version = self
            .0
            .write_fenced(
                &claim.partition,
                &next.id,
                &next.owner,
                next.retained_until,
                &next,
                Some(claim.record.version),
            )
            .await?;
        claim.record = next;
        Ok(())
    }

    pub async fn guard(&self, claim: &ReplyClaim) -> Result<(), ExecutionError> {
        self.0.guard_partition(&claim.partition).await?;
        if !claim.live_partition() || claim.until() <= (self.0.clock)() {
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
        self.finish_observed(claim, receipt, AnswerDispatch::Indeterminate)
            .await
    }

    pub async fn finish_observed(
        &self,
        claim: &ReplyClaim,
        receipt: &IntentReceipt,
        dispatch: AnswerDispatch,
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
                .finish(
                    &claim.partition,
                    &claim.record,
                    claim.attempt(),
                    &receipt,
                    dispatch,
                )
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
        if let Err(error) = self
            .0
            .write_fenced(
                &claim.partition,
                &next.id,
                &next.owner,
                next.retained_until,
                &next,
                Some(claim.record.version),
            )
            .await
        {
            if self.0.partitions.is_none()
                || !matches!(error, ExecutionError::Busy | ExecutionError::LostLease)
            {
                return Err(error);
            }
            // A parent can close after reading the durable keyed receipt but
            // before this guard CAS. Read-only confirmation must not turn that
            // exact saved outcome into a failed tool response or overwrite close.
            if let Some((stored, version)) = self.0.read::<Record>(&next.id).await? {
                let attempt = match claim.kind {
                    Kind::Tool => &stored.tool,
                    Kind::Automatic => &stored.automatic,
                };
                if stored.closed
                    && version > claim.record.version
                    && stored.target == next.target
                    && stored.root == next.root
                    && stored.owner == next.owner
                    && stored.request_hash == next.request_hash
                    && stored.retained_until == next.retained_until
                    && attempt.as_ref().is_some_and(|a| {
                        a.claim == claim.attempt().claim
                            && a.operation == claim.attempt().operation
                            && a.content_hash == claim.attempt().content_hash
                    })
                    && attempt
                        .as_ref()
                        .and_then(|a| a.receipt.as_ref())
                        .map(storage::bytes)
                        .transpose()?
                        == Some(storage::bytes(&receipt)?)
                {
                    return Ok(receipt);
                }
            }
            return Err(error);
        }
        Ok(receipt)
    }
}
