//! One immutable payload binding shared by reservation and refusal writers.
use super::{
    storage,
    tool_operation::OperationStore,
    tool_reply_store::{Record, ReplyStore},
    AcceptedAnswerIdentity, ExecutionError, Journal,
};
use crate::delivery::intents::IntentReceipt;
use crate::ingress::routing_store::{AdmittedPlan, StoredTarget};
use crate::replicas::partition::PartitionLease;
use cmaeg_core::message::OutboundMessage;
use serde::{Deserialize, Serialize};

const KIND: &str = "accepted_answer_payload_binding_v1";

#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    kind: String,
    target: String,
    root: String,
    owner: String,
    request_hash: String,
    retained_until: i64,
    answer: AcceptedAnswerIdentity,
    content_hash: String,
}

enum Writer<'a> {
    Partition(&'a Option<PartitionLease>),
    DelayedIntake,
}

impl Journal {
    pub(super) async fn admit_delayed_answer_payload(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        answer: &AcceptedAnswerIdentity,
        message: &OutboundMessage,
    ) -> Result<Option<IntentReceipt>, ExecutionError> {
        let record = ReplyStore(self.clone(), None).load(plan, target).await?;
        let (operation, hash) = answer.operation(target, message)?;
        // Bind before reserving a FIFO slot. A losing payload cannot strand
        // delayed work or take the slot needed to replay the winning answer.
        self.bind_answer(Writer::DelayedIntake, &record, &operation, &hash, answer)
            .await?;
        OperationStore(self.clone())
            .lookup(&record, &operation, &hash)
            .await
    }

    pub(super) async fn bind_answer_payload(
        &self,
        partition: &Option<PartitionLease>,
        record: &Record,
        operation: &str,
        content_hash: &str,
        answer: &AcceptedAnswerIdentity,
    ) -> Result<(), ExecutionError> {
        self.bind_answer(
            Writer::Partition(partition),
            record,
            operation,
            content_hash,
            answer,
        )
        .await
    }

    async fn bind_answer(
        &self,
        writer: Writer<'_>,
        record: &Record,
        operation: &str,
        content_hash: &str,
        answer: &AcceptedAnswerIdentity,
    ) -> Result<(), ExecutionError> {
        if answer.operation_id(&record.target)? != operation
            || !super::tool_operation::valid_hash(content_hash)
        {
            return Err(ExecutionError::Invalid);
        }
        let id = storage::identifier(&(KIND, &record.target));
        let expected = Binding {
            kind: KIND.into(),
            target: record.target.clone(),
            root: record.root.clone(),
            owner: record.owner.clone(),
            request_hash: record.request_hash.clone(),
            retained_until: record.retained_until,
            answer: answer.clone(),
            content_hash: content_hash.into(),
        };
        if let Some(saved) = self.read::<Binding>(&id).await? {
            return matches_binding(saved, expected);
        }
        let written = match writer {
            Writer::Partition(partition) => {
                self.write_fenced(
                    partition,
                    &id,
                    &record.owner,
                    record.retained_until,
                    &expected,
                    None,
                )
                .await
            }
            // Intake has already authenticated the original target. This
            // create-only write holds payload identity and grants no custody.
            Writer::DelayedIntake => {
                self.write(&id, &record.owner, record.retained_until, &expected, None)
                    .await
            }
        };
        match written {
            Ok(1) => Ok(()),
            Ok(_) => Err(ExecutionError::Corrupt),
            Err(
                error @ (ExecutionError::Busy
                | ExecutionError::LostLease
                | ExecutionError::Uncertain
                | ExecutionError::Unavailable),
            ) => match self.read::<Binding>(&id).await? {
                Some(saved) => matches_binding(saved, expected),
                None => Err(error),
            },
            Err(error) => Err(error),
        }
    }
}

fn matches_binding(
    (saved, version): (Binding, u64),
    expected: Binding,
) -> Result<(), ExecutionError> {
    if version != 1
        || saved.kind != expected.kind
        || saved.target != expected.target
        || saved.root != expected.root
        || saved.owner != expected.owner
        || saved.request_hash != expected.request_hash
        || saved.retained_until != expected.retained_until
    {
        return Err(ExecutionError::Corrupt);
    }
    if saved.answer != expected.answer || saved.content_hash != expected.content_hash {
        return Err(ExecutionError::IdempotencyConflict);
    }
    Ok(())
}
