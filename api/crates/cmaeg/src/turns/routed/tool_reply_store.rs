//! A separate CAS fence lets tool calls and the execution lease coordinate
//! without either writer overwriting the other's execution/projection state.
use super::{
    storage,
    tool_operation::{valid_hash, OperationStore},
    ExecutionError, Journal,
};
use crate::{
    delivery::{
        intents::{DeliveryOutcome, IntentReceipt},
        retained_receipt::StoredReceipt,
    },
    ingress::routing_store::{AdmittedPlan, StoredTarget},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
}
impl ReplyClaim {
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
pub(super) struct ReplyStore(pub Journal);
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
                .write(
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
            .write(
                &record.id,
                &record.owner,
                record.retained_until,
                &record,
                (record.version > 0).then_some(record.version),
            )
            .await?;
        Ok(Admission::Claimed(Box::new(ReplyClaim { record, kind })))
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
            .write(
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
            .prepare(&record, record.tool.as_ref().unwrap())
            .await?;
        Ok(Admission::Claimed(Box::new(ReplyClaim {
            record,
            kind: Kind::Tool,
        })))
    }
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
    fn unsettled(&self, record: &Record) -> Option<ToolReplyStatus> {
        let mut result = None;
        for attempt in [&record.tool, &record.automatic].into_iter().flatten() {
            match &attempt.receipt {
                None if attempt.until > (self.0.clock)() => result = Some(ToolReplyStatus::Pending),
                None => return Some(ToolReplyStatus::Unknown),
                Some(receipt) if matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }) => {
                    return Some(ToolReplyStatus::Unknown);
                }
                _ => {}
            }
        }
        result
    }
    async fn load(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Record, ExecutionError> {
        if plan.retained_until <= (self.0.clock)() {
            return Err(ExecutionError::Expired);
        }
        let id = storage::identifier(&("ordinary-route-reply-guard-v1", &target.id));
        let expected = Record {
            kind: "ordinary_route_reply_guard_v1".into(),
            id: id.clone(),
            target: target.id.clone(),
            root: plan.admission_id.clone(),
            owner: plan.identity.owner.clone(),
            request_hash: plan.request_hash.clone(),
            retained_until: plan.retained_until,
            confirmed_tool: false,
            operation_keys: BTreeSet::new(),
            closed: false,
            tool: None,
            automatic: None,
            version: 0,
        };
        let Some((mut record, version)) = self.0.read::<Record>(&id).await? else {
            return Ok(expected);
        };
        if record.kind != expected.kind
            || record.id != id
            || record.target != expected.target
            || record.root != expected.root
            || record.owner != expected.owner
            || record.request_hash != expected.request_hash
            || record.retained_until != expected.retained_until
            || storage::bytes(&record)?.len() > 40 * 1024
            || record.operation_keys.len() > 64
            || record.operation_keys.iter().any(|key| !valid_hash(key))
            || [&record.tool, &record.automatic]
                .into_iter()
                .flatten()
                .any(|attempt| {
                    uuid::Uuid::parse_str(&attempt.claim).is_err()
                        || !valid_hash(&attempt.operation)
                        || attempt.content_hash.as_ref().is_some_and(|hash| {
                            !valid_hash(hash) || !record.operation_keys.contains(&attempt.operation)
                        })
                })
        {
            return Err(ExecutionError::Corrupt);
        }
        record.version = version;
        if let Some(attempt) = &record.tool {
            if attempt.content_hash.is_some() && attempt.receipt.is_none() {
                if let Some(receipt) = OperationStore(self.0.clone())
                    .reconcile(&record, attempt)
                    .await?
                {
                    record.confirmed_tool |=
                        matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. });
                    record.tool.as_mut().unwrap().receipt = Some(receipt);
                    record.version = self
                        .0
                        .write(
                            &record.id,
                            &record.owner,
                            record.retained_until,
                            &record,
                            Some(version),
                        )
                        .await?;
                }
            }
        }
        Ok(record)
    }
}
