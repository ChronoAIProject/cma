//! A separate CAS fence lets tool calls and the execution lease coordinate
//! without either writer overwriting the other's execution/projection state.
use super::{
    storage,
    tool_operation::{valid_hash, OperationStore},
    ExecutionError, Journal,
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
    #[serde(default)]
    pub(super) closed: bool,
    pub(super) tool: Option<Attempt>,
    pub(super) automatic: Option<Attempt>,
    #[serde(skip)]
    pub(super) version: u64,
}
#[derive(Clone)]
pub(super) struct ReplyClaim {
    record: Record,
    kind: Kind,
    partition: Option<PartitionLease>,
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
    pub async fn status(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<ToolReplyStatus, ExecutionError> {
        let record = self.load(plan, target).await?;
        if let Some(status) = self.unsettled(&record) {
            return Ok(status);
        }
        if record.confirmed_tool {
            return Ok(ToolReplyStatus::Confirmed);
        }
        Ok(ToolReplyStatus::None)
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
        let mut record = self.load(plan, target).await?;
        if record.operation_keys.contains(&operation) {
            if let Some(receipt) = OperationStore(self.0.clone())
                .lookup(&record, &operation, &content_hash)
                .await?
            {
                return Ok(Admission::Replay(receipt));
            }
            let attempt = record
                .tool
                .as_ref()
                .filter(|attempt| attempt.operation == operation)
                .ok_or(ExecutionError::Corrupt)?;
            if attempt.content_hash.as_ref() != Some(&content_hash) {
                return Err(ExecutionError::IdempotencyConflict);
            }
            return Ok(if attempt.until > (self.0.clock)() {
                Admission::Pending
            } else {
                Admission::Unknown
            });
        }
        if let Some(status) = self.unsettled(&record) {
            return Ok(if status == ToolReplyStatus::Pending {
                Admission::Pending
            } else {
                Admission::Unknown
            });
        }
        if record.closed {
            return Ok(Admission::Closed);
        }
        if record.operation_keys.len() >= 64 {
            return Err(ExecutionError::OperationLimit);
        }
        record.operation_keys.insert(operation.clone());
        record.tool = Some(Attempt {
            claim: uuid::Uuid::new_v4().to_string(),
            operation,
            content_hash: Some(content_hash),
            until: (self.0.clock)() + 30,
            receipt: None,
        });
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
        OperationStore(self.0.clone())
            .prepare(&self.1, &record, record.tool.as_ref().unwrap())
            .await?;
        Ok(Admission::Claimed(Box::new(ReplyClaim {
            record,
            kind: Kind::Tool,
            partition: self.1.clone(),
        })))
    }
}
