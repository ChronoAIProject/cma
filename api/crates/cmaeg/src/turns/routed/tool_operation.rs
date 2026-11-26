//! Retained receipts for explicit direct-reply keys; the shared CAS guard admits
//! at most 64 such records per original target before any external effect.
use super::{
    answer_dispatch::AnswerDispatch,
    answer_receipt::AnswerReceiptResult,
    storage,
    tool_reply_store::{Attempt, Record},
    AcceptedAnswerIdentity, ExecutionError, Journal,
};
use crate::delivery::{intents::IntentReceipt, retained_receipt::StoredReceipt};
use crate::replicas::partition::PartitionLease;
use serde::{Deserialize, Serialize};
const KIND: &str = "ordinary_route_tool_operation_v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    kind: String,
    id: String,
    target: String,
    root: String,
    owner: String,
    request_hash: String,
    retained_until: i64,
    operation: String,
    content_hash: String,
    claim: String,
    receipt: Option<StoredReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    answer: Option<AcceptedAnswerIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    answer_settlement: Option<AnswerSettlement>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerSettlement {
    dispatch: AnswerDispatch,
    settled_at: i64,
}
#[derive(Clone)]
pub(super) struct OperationStore(pub Journal);
impl OperationStore {
    pub async fn recover_answer(
        &self,
        partition: &Option<PartitionLease>,
        record: &Record,
        attempt: &Attempt,
    ) -> Result<Option<StoredReceipt>, ExecutionError> {
        let Some(hash) = attempt.content_hash.as_deref() else {
            return Ok(None);
        };
        let Some(entry) = self.read(record, &attempt.operation, Some(hash)).await? else {
            return Ok(None);
        };
        if entry.answer.is_none() {
            return Ok(None);
        }
        if entry.claim != attempt.claim {
            return Err(ExecutionError::Corrupt);
        }
        if let Some(receipt) = entry.receipt {
            return Ok(Some(receipt));
        }
        if attempt.until > (self.0.clock)() {
            return Err(ExecutionError::Busy);
        }
        let outcome = crate::delivery::intents::DeliveryOutcome::Unknown {
            reason: "accepted_answer_dispatch_unconfirmed".into(),
        };
        let receipt = crate::delivery::durable::receipt_for(outcome).into();
        self.finish(
            partition,
            record,
            attempt,
            &receipt,
            AnswerDispatch::Indeterminate,
        )
        .await?;
        Ok(Some(receipt))
    }

    pub async fn retained_receipt(
        &self,
        record: &Record,
        operation: &str,
        content_hash: &str,
    ) -> Result<Option<StoredReceipt>, ExecutionError> {
        Ok(self
            .read(record, operation, Some(content_hash))
            .await?
            .and_then(|entry| entry.receipt))
    }

    pub async fn lookup(
        &self,
        record: &Record,
        operation: &str,
        content_hash: &str,
    ) -> Result<Option<IntentReceipt>, ExecutionError> {
        let Some(entry) = self.read(record, operation, Some(content_hash)).await? else {
            return Ok(None);
        };
        match entry.receipt {
            Some(receipt) => self
                .0
                .receipts(&entry.operation, &record.owner, record.retained_until)
                .load(&receipt)
                .await
                .map(Some)
                .map_err(|_| ExecutionError::Unavailable),
            None => Ok(None),
        }
    }
    pub async fn prepare(
        &self,
        partition: &Option<PartitionLease>,
        record: &Record,
        attempt: &Attempt,
        answer: Option<&AcceptedAnswerIdentity>,
    ) -> Result<(), ExecutionError> {
        let content_hash = attempt
            .content_hash
            .as_ref()
            .ok_or(ExecutionError::Invalid)?;
        let entry = Operation {
            kind: KIND.into(),
            id: id(record, &attempt.operation),
            target: record.target.clone(),
            root: record.root.clone(),
            owner: record.owner.clone(),
            request_hash: record.request_hash.clone(),
            retained_until: record.retained_until,
            operation: attempt.operation.clone(),
            content_hash: content_hash.clone(),
            claim: attempt.claim.clone(),
            receipt: None,
            answer: answer.cloned(),
            answer_settlement: None,
        };
        self.0
            .write_fenced(
                partition,
                &entry.id,
                &entry.owner,
                entry.retained_until,
                &entry,
                None,
            )
            .await?;
        Ok(())
    }
    pub async fn finish(
        &self,
        partition: &Option<PartitionLease>,
        record: &Record,
        attempt: &Attempt,
        receipt: &StoredReceipt,
        dispatch: AnswerDispatch,
    ) -> Result<(), ExecutionError> {
        let mut entry = self
            .read(
                record,
                &attempt.operation,
                Some(
                    attempt
                        .content_hash
                        .as_deref()
                        .ok_or(ExecutionError::Invalid)?,
                ),
            )
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        if entry.claim != attempt.claim {
            return Err(ExecutionError::LostLease);
        }
        if let Some(saved) = &entry.receipt {
            return if storage::bytes(saved)? == storage::bytes(receipt)? {
                Ok(())
            } else {
                Err(ExecutionError::Corrupt)
            };
        }
        entry.receipt = Some(receipt.clone());
        entry.answer_settlement = entry.answer.as_ref().map(|_| AnswerSettlement {
            dispatch,
            settled_at: (self.0.clock)(),
        });
        self.0
            .write_fenced(
                partition,
                &entry.id,
                &entry.owner,
                entry.retained_until,
                &entry,
                Some(1),
            )
            .await?;
        Ok(())
    }
    pub async fn reconcile(
        &self,
        record: &Record,
        attempt: &Attempt,
    ) -> Result<Option<StoredReceipt>, ExecutionError> {
        let Some(content_hash) = &attempt.content_hash else {
            return Ok(None);
        };
        let Some(entry) = self
            .read(record, &attempt.operation, Some(content_hash))
            .await?
        else {
            return Ok(None);
        };
        if entry.claim != attempt.claim {
            return Err(ExecutionError::Corrupt);
        }
        Ok(entry.receipt)
    }
    pub async fn answer(
        &self,
        record: &Record,
        identity: &AcceptedAnswerIdentity,
    ) -> Result<Option<AnswerReceiptResult>, ExecutionError> {
        let operation = identity.operation_id(&record.target)?;
        let Some(entry) = self.read(record, &operation, None).await? else {
            return Ok(None);
        };
        if entry.answer.as_ref() != Some(identity) {
            return Err(ExecutionError::IdempotencyConflict);
        }
        Ok(entry
            .receipt
            .as_ref()
            .zip(entry.answer_settlement)
            .map(|(receipt, settlement)| {
                AnswerReceiptResult::new(
                    operation,
                    receipt,
                    settlement.dispatch,
                    settlement.settled_at,
                )
            }))
    }
    async fn read(
        &self,
        record: &Record,
        operation: &str,
        content_hash: Option<&str>,
    ) -> Result<Option<Operation>, ExecutionError> {
        let expected = id(record, operation);
        let Some((entry, version)) = self.0.read::<Operation>(&expected).await? else {
            return Ok(None);
        };
        if entry.kind != KIND
            || entry.id != expected
            || entry.target != record.target
            || entry.root != record.root
            || entry.owner != record.owner
            || entry.request_hash != record.request_hash
            || entry.retained_until != record.retained_until
            || entry.operation != operation
            || uuid::Uuid::parse_str(&entry.claim).is_err()
            || !valid_hash(&entry.content_hash)
            || entry.answer.as_ref().is_some_and(|answer| {
                answer.operation_id(&record.target).ok().as_deref() != Some(operation)
            })
            || entry.answer_settlement.is_some()
                != (entry.answer.is_some() && entry.receipt.is_some())
            || entry.answer_settlement.as_ref().is_some_and(|settlement| {
                settlement.settled_at <= 0 || settlement.settled_at >= record.retained_until
            })
            || version != if entry.receipt.is_some() { 2 } else { 1 }
            || entry.receipt.as_ref().is_some_and(|receipt| {
                storage::bytes(receipt).map_or(true, |b| b.len() > 16 * 1024)
            })
        {
            return Err(ExecutionError::Corrupt);
        }
        if content_hash.is_some_and(|hash| entry.content_hash != hash) {
            return Err(ExecutionError::IdempotencyConflict);
        }
        Ok(Some(entry))
    }
}
fn id(record: &Record, operation: &str) -> String {
    storage::identifier(&(KIND, &record.target, operation))
}
pub(super) fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
