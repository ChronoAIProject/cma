//! Retained receipts for explicit direct-reply keys; the shared CAS guard admits
//! at most 64 such records per original target before any external effect.
use super::{
    storage,
    tool_reply_store::{Attempt, Record},
    ExecutionError, Journal,
};
use crate::delivery::{intents::IntentReceipt, retained_receipt::StoredReceipt};
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
}
#[derive(Clone)]
pub(super) struct OperationStore(pub Journal);
impl OperationStore {
    pub async fn lookup(
        &self,
        record: &Record,
        operation: &str,
        content_hash: &str,
    ) -> Result<Option<IntentReceipt>, ExecutionError> {
        let Some(entry) = self.read(record, operation, content_hash).await? else {
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
    pub async fn prepare(&self, record: &Record, attempt: &Attempt) -> Result<(), ExecutionError> {
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
        };
        self.0
            .write(&entry.id, &entry.owner, entry.retained_until, &entry, None)
            .await?;
        Ok(())
    }
    pub async fn finish(
        &self,
        record: &Record,
        attempt: &Attempt,
        receipt: &StoredReceipt,
    ) -> Result<(), ExecutionError> {
        let mut entry = self
            .read(
                record,
                &attempt.operation,
                attempt
                    .content_hash
                    .as_deref()
                    .ok_or(ExecutionError::Invalid)?,
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
        self.0
            .write(
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
        let Some(entry) = self.read(record, &attempt.operation, content_hash).await? else {
            return Ok(None);
        };
        if entry.claim != attempt.claim {
            return Err(ExecutionError::Corrupt);
        }
        Ok(entry.receipt)
    }
    async fn read(
        &self,
        record: &Record,
        operation: &str,
        content_hash: &str,
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
            || version != if entry.receipt.is_some() { 2 } else { 1 }
            || entry.receipt.as_ref().is_some_and(|receipt| {
                storage::bytes(receipt).map_or(true, |b| b.len() > 16 * 1024)
            })
        {
            return Err(ExecutionError::Corrupt);
        }
        if entry.content_hash != content_hash {
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
