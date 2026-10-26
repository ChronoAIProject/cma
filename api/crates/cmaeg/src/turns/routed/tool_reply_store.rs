//! A separate CAS fence lets tool calls and the execution lease coordinate
//! without either writer overwriting the other's execution/projection state.
use super::{storage, ExecutionError, Journal};
use crate::{
    delivery::intents::{DeliveryOutcome, IntentReceipt},
    ingress::routing_store::{AdmittedPlan, StoredTarget},
};
use serde::{Deserialize, Serialize};

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
struct Attempt {
    claim: String,
    operation: String,
    until: i64,
    receipt: Option<IntentReceipt>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    kind: String,
    id: String,
    target: String,
    root: String,
    owner: String,
    request_hash: String,
    retained_until: i64,
    confirmed_tool: bool,
    #[serde(default)]
    closed: bool,
    tool: Option<Attempt>,
    automatic: Option<Attempt>,
    #[serde(skip)]
    version: u64,
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
                        return Ok(Admission::Replay(receipt.clone()));
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
    ) -> Result<(), ExecutionError> {
        self.guard(claim).await?;
        let mut next = claim.record.clone();
        let receipt = if storage::bytes(receipt)?.len() > 16 * 1024 {
            crate::delivery::durable::receipt_for(DeliveryOutcome::Unknown {
                reason: "reply_receipt_too_large".into(),
            })
        } else {
            receipt.clone()
        };
        if matches!(claim.kind, Kind::Tool)
            && matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. })
        {
            next.confirmed_tool = true;
        }
        let attempt = match claim.kind {
            Kind::Tool => &mut next.tool,
            Kind::Automatic => &mut next.automatic,
        };
        attempt.as_mut().unwrap().receipt = Some(receipt);
        self.0
            .write(
                &next.id,
                &next.owner,
                next.retained_until,
                &next,
                Some(claim.record.version),
            )
            .await?;
        Ok(())
    }
    fn unsettled(&self, record: &Record) -> Option<ToolReplyStatus> {
        let mut result = None;
        for attempt in [&record.tool, &record.automatic].into_iter().flatten() {
            match &attempt.receipt {
                None if attempt.until > (self.0.clock)() => result = Some(ToolReplyStatus::Pending),
                None
                | Some(IntentReceipt {
                    delivery: DeliveryOutcome::Unknown { .. },
                    ..
                }) => return Some(ToolReplyStatus::Unknown),
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
            || [&record.tool, &record.automatic]
                .into_iter()
                .flatten()
                .any(|attempt| {
                    uuid::Uuid::parse_str(&attempt.claim).is_err() || attempt.operation.len() != 64
                })
        {
            return Err(ExecutionError::Corrupt);
        }
        record.version = version;
        Ok(record)
    }
}
