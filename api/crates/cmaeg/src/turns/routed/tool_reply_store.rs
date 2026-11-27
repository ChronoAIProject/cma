//! A separate CAS fence lets tool calls and the execution lease coordinate
//! without either writer overwriting the other's execution/projection state.
use super::{
    storage,
    tool_operation::{valid_hash, OperationStore},
    AcceptedAnswerIdentity, ExecutionError, Journal,
};
use crate::replicas::partition::PartitionLease;
use crate::{
    delivery::{
        intents::{DeliveryOutcome, IntentReceipt},
        retained_receipt::StoredReceipt,
    },
    ingress::routing_store::{AdmittedPlan, StoredTarget},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod delayed;
mod legacy;
mod replay;
mod retained;
mod settlement;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolReplyStatus {
    None,
    Confirmed,
    Pending,
    Unknown,
}
#[derive(Clone, Copy)]
pub(super) enum Kind {
    Tool,
    Automatic,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attempt {
    pub(super) claim: String,
    pub(super) operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) answer: Option<AcceptedAnswerIdentity>,
    pub(super) until: i64,
    pub(super) receipt: Option<StoredReceipt>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub(super) kind: String,
    pub(super) id: String,
    pub(super) target: String,
    pub(super) root: String,
    pub(super) owner: String,
    pub(super) request_hash: String,
    pub(super) retained_until: i64,
    pub(super) confirmed_tool: bool,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(super) operation_keys: BTreeSet<String>,
    /// One pre-replica unkeyed confirmation, archived before displacing its slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) legacy_confirmation: Option<String>,
    #[serde(default)]
    pub(super) closed: bool,
    pub(super) tool: Option<Attempt>,
    pub(super) automatic: Option<Attempt>,
    #[serde(skip)]
    pub(super) version: u64,
}
impl Record {
    fn keyed_attempt(&self, operation: &str) -> Result<&Attempt, ExecutionError> {
        [&self.tool, &self.automatic]
            .into_iter()
            .flatten()
            .find(|attempt| attempt.operation == operation)
            .ok_or(ExecutionError::Corrupt)
    }
}
#[derive(Clone)]
pub(super) struct ReplyClaim {
    record: Record,
    kind: Kind,
    pub(super) partition: Option<PartitionLease>,
}
impl ReplyClaim {
    pub fn live_partition(&self) -> bool {
        self.partition.as_ref().is_none_or(PartitionLease::live)
    }
    pub fn until(&self) -> i64 {
        self.attempt().until
    }
    fn attempt(&self) -> &Attempt {
        match self.kind {
            Kind::Tool => self.record.tool.as_ref(),
            Kind::Automatic => self.record.automatic.as_ref(),
        }
        .unwrap()
    }
}
pub(super) enum Admission {
    Claimed(Box<ReplyClaim>),
    Replay(IntentReceipt),
    Suppressed,
    Pending,
    Unknown,
    Closed,
}
#[derive(Clone)]
pub(super) struct ReplyStore(pub Journal, pub Option<PartitionLease>);
impl ReplyStore {
    pub(crate) async fn operator_record(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<(serde_json::Value, u64), ExecutionError> {
        let validated = self.load(plan, target).await?;
        match self.0.read::<serde_json::Value>(&validated.id).await? {
            Some((raw, version)) => Ok((raw, version)),
            None => Ok((
                serde_json::to_value(&validated).map_err(|_| ExecutionError::Corrupt)?,
                0,
            )),
        }
    }

    pub async fn status(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<ToolReplyStatus, ExecutionError> {
        self.observation(plan, target).await.map(|value| value.0)
    }
    /// One validated snapshot for status and deadline, including already-saved
    /// receipts. No execution-lease check is required for this read-only view.
    pub async fn observation(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<(ToolReplyStatus, Option<i64>), ExecutionError> {
        let record = self.load(plan, target).await?;
        let now = (self.0.clock)();
        let pending = [record.tool.as_ref(), record.automatic.as_ref()]
            .into_iter()
            .flatten()
            .filter(|attempt| attempt.receipt.is_none() && attempt.until > now)
            .map(|attempt| attempt.until)
            .min();
        let status = self
            .unsettled_at(&record, now)
            .unwrap_or(if record.confirmed_tool {
                ToolReplyStatus::Confirmed
            } else {
                ToolReplyStatus::None
            });
        Ok((status, pending))
    }
    /// The final cursor may advance only after this CAS excludes late tool
    /// admissions. A failed close never discards an already settled receipt.
    pub async fn close(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<ToolReplyStatus, ExecutionError> {
        let mut record = self.load(plan, target).await?;
        if let Some(status) = self.unsettled(&record) {
            return Ok(status);
        }
        if !record.closed {
            record.closed = true;
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
        Ok(if record.confirmed_tool {
            ToolReplyStatus::Confirmed
        } else {
            ToolReplyStatus::None
        })
    }
    pub async fn admit(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        kind: Kind,
        operation: String,
    ) -> Result<Admission, ExecutionError> {
        let mut record = self.load(plan, target).await?;
        if let Some(status) = self.unsettled(&record) {
            return Ok(match status {
                ToolReplyStatus::Pending => Admission::Pending,
                _ => Admission::Unknown,
            });
        }
        let previous = match kind {
            Kind::Tool => &record.tool,
            Kind::Automatic => &record.automatic,
        };
        if let Some(previous) = previous {
            if previous.operation == operation {
                if let Some(receipt) = &previous.receipt {
                    if matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }) {
                        return Ok(Admission::Replay(
                            self.0
                                .receipts(&previous.operation, &record.owner, record.retained_until)
                                .load(receipt)
                                .await
                                .map_err(|_| ExecutionError::Unavailable)?,
                        ));
                    }
                }
            }
        }
        if record.closed {
            return Ok(Admission::Closed);
        }
        if matches!(kind, Kind::Automatic) && record.confirmed_tool {
            return Ok(Admission::Suppressed);
        }
        let attempt = Some(Attempt {
            claim: uuid::Uuid::new_v4().to_string(),
            operation,
            content_hash: None,
            answer: None,
            until: (self.0.clock)() + 30,
            receipt: None,
        });
        match kind {
            Kind::Tool => record.tool = attempt,
            Kind::Automatic => record.automatic = attempt,
        }
        record.version = self
            .0
            .write_fenced(
                &self.1,
                &record.id,
                &record.owner,
                record.retained_until,
                &record,
                (record.version > 0).then_some(record.version),
            )
            .await?;
        Ok(Admission::Claimed(Box::new(ReplyClaim {
            record,
            kind,
            partition: self.1.clone(),
        })))
    }
    pub async fn admit_keyed(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        operation: String,
        content_hash: String,
    ) -> Result<Admission, ExecutionError> {
        self.admit_keyed_inner(
            plan,
            target,
            (operation, content_hash, None),
            Kind::Tool,
            self.0.partitions.is_some(),
            false,
        )
        .await
    }
    pub async fn admit_keyed_answer(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        key: (String, String, Option<&AcceptedAnswerIdentity>),
    ) -> Result<Admission, ExecutionError> {
        self.admit_keyed_inner(plan, target, key, Kind::Automatic, true, false)
            .await
    }

    pub async fn refuse_answer(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        operation: &str,
        content_hash: &str,
        answer: &AcceptedAnswerIdentity,
        reason: &str,
    ) -> Result<IntentReceipt, ExecutionError> {
        let record = self.load(plan, target).await?;
        self.0
            .bind_answer_payload(&self.1, &record, operation, content_hash, answer)
            .await?;
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
    async fn admit_keyed_inner(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        (operation, content_hash, answer): (String, String, Option<&AcceptedAnswerIdentity>),
        kind: Kind,
        preserve_legacy: bool,
        allow_closed: bool,
    ) -> Result<Admission, ExecutionError> {
        let mut record = self.load(plan, target).await?;
        let operations = OperationStore(self.0.clone());
        if let Some(answer) = answer {
            self.0
                .bind_answer_payload(&self.1, &record, &operation, &content_hash, answer)
                .await?;
        }
        if answer.is_some() || record.operation_keys.contains(&operation) {
            if let Some(receipt) = operations
                .lookup(&record, &operation, &content_hash)
                .await?
            {
                return Ok(Admission::Replay(receipt));
            }
        }
        if record.operation_keys.contains(&operation) {
            let attempt = record.keyed_attempt(&operation)?;
            if attempt.content_hash.as_ref() != Some(&content_hash)
                || attempt.answer.as_ref() != answer
            {
                return Err(ExecutionError::IdempotencyConflict);
            }
            if attempt.until > (self.0.clock)() {
                return Ok(Admission::Pending);
            }
            if let Some(receipt) = operations.recover_answer(&self.1, &record, attempt).await? {
                return Ok(Admission::Replay(
                    self.0
                        .receipts(&operation, &record.owner, record.retained_until)
                        .load(&receipt)
                        .await
                        .map_err(|_| ExecutionError::Unavailable)?,
                ));
            }
            return Ok(Admission::Unknown);
        }
        if let Some(receipt) = self.exact_confirmation(&record, &operation, true).await? {
            return Ok(Admission::Replay(receipt));
        }
        if let Some(status) = self.unsettled(&record) {
            return Ok(if status == ToolReplyStatus::Pending {
                Admission::Pending
            } else {
                Admission::Unknown
            });
        }
        if record.closed && !allow_closed {
            return Ok(Admission::Closed);
        }
        if preserve_legacy {
            self.preserve_legacy(&mut record).await?;
        }
        if record.operation_keys.len() + usize::from(record.legacy_confirmation.is_some()) >= 64 {
            return Err(ExecutionError::OperationLimit);
        }
        record.operation_keys.insert(operation.clone());
        let attempt = Attempt {
            claim: uuid::Uuid::new_v4().to_string(),
            operation: operation.clone(),
            content_hash: Some(content_hash),
            answer: answer.cloned(),
            until: (self.0.clock)() + 30,
            receipt: None,
        };
        match kind {
            Kind::Tool => record.tool = Some(attempt),
            Kind::Automatic => record.automatic = Some(attempt),
        }
        if storage::bytes(&record)?.len() > 40 * 1024 {
            return Err(ExecutionError::OperationLimit);
        }
        record.version = self
            .0
            .write_fenced(
                &self.1,
                &record.id,
                &record.owner,
                record.retained_until,
                &record,
                (record.version > 0).then_some(record.version),
            )
            .await?;
        // Only the CAS winner creates a retained key record; failed competing
        // admissions cannot consume records outside the 64-operation bound.
        operations
            .prepare(&self.1, &record, record.keyed_attempt(&operation)?)
            .await?;
        Ok(Admission::Claimed(Box::new(ReplyClaim {
            record,
            kind,
            partition: self.1.clone(),
        })))
    }
}

#[cfg(test)]
mod legacy_tests;
