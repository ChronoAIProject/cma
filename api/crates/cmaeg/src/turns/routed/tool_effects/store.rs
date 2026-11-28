use super::{
    super::{storage, AcceptedAnswerIdentity, ExecutionError, Journal},
    model::*,
};
use crate::{
    delivery::intents::IntentReceipt,
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    replicas::partition::PartitionLease,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Chunk {
    kind: String,
    work: String,
    payload_hash: String,
    index: usize,
    data: String,
}

#[derive(Clone)]
pub(super) struct Store(pub Journal);
impl Store {
    pub async fn read(
        &self,
        id: &str,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Option<Root>, ExecutionError> {
        let Some((mut root, version)) = self.0.read::<Root>(id).await? else {
            return Ok(None);
        };
        root.version = version;
        root.validate(plan, target)?;
        if root.id != id {
            return Err(ExecutionError::Corrupt);
        }
        Ok(Some(root))
    }
    pub async fn admit(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        key: &str,
        payload: &Payload,
    ) -> Result<Root, ExecutionError> {
        self.admit_inner(plan, target, key, payload, None, true)
            .await
    }
    pub async fn admit_answer(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        payload: &Payload,
        answer: AcceptedAnswerIdentity,
    ) -> Result<Root, ExecutionError> {
        self.admit_inner(plan, target, "", payload, Some(answer), true)
            .await
    }
    #[cfg(test)]
    pub async fn admit_legacy(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        key: &str,
        payload: &Payload,
    ) -> Result<Root, ExecutionError> {
        self.admit_inner(plan, target, key, payload, None, false)
            .await
    }
    async fn admit_inner(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        key: &str,
        payload: &Payload,
        answer: Option<AcceptedAnswerIdentity>,
        select_lane: bool,
    ) -> Result<Root, ExecutionError> {
        let (operation, content_hash) = match &answer {
            Some(answer) => answer.operation(target, &payload.message)?,
            None => (
                operation(target, payload, key)?,
                storage::hash(&storage::bytes(&payload.message)?),
            ),
        };
        let id = work_id(&operation);
        if let Some(root) = self.read(&id, plan, target).await? {
            return same(root, &operation, &content_hash);
        }
        let bytes = storage::bytes(payload)?;
        let root = Root {
            kind: KIND.into(),
            id,
            identity: plan.identity.clone(),
            admission: plan.admission_id.clone(),
            target: target.id.clone(),
            request_hash: plan.request_hash.clone(),
            retained_until: plan.retained_until,
            operation: operation.clone(),
            content_hash: content_hash.clone(),
            payload_hash: storage::hash(&bytes),
            bytes: bytes.len(),
            expires_at: payload.capability.expires_at,
            answer,
            state: State::Queued,
            version: 1,
        };
        root.validate(plan, target)?;
        if select_lane {
            match self
                .0
                .partitions
                .as_ref()
                .ok_or(ExecutionError::Unsupported)?
                .reserve_delayed(plan, target, &root.id, &root.content_hash)
                .await?
            {
                crate::replicas::DelayedReservation::Ready => {}
                crate::replicas::DelayedReservation::Conflict => {
                    return Err(ExecutionError::IdempotencyConflict)
                }
            }
        }
        for (index, data) in bytes.chunks(CHUNK_BYTES).enumerate() {
            let chunk = Chunk {
                kind: format!("{KIND}_chunk"),
                work: root.id.clone(),
                payload_hash: root.payload_hash.clone(),
                index,
                data: STANDARD.encode(data),
            };
            self.0
                .write(
                    &root.chunk_id(index),
                    &root.identity.owner,
                    root.retained_until,
                    &chunk,
                    None,
                )
                .await?;
        }
        let write = if select_lane {
            self.0
                .admit_delayed_root(plan, target, &root.id, &root)
                .await
        } else {
            self.0
                .write_effect_root(&root.id, &root.identity.owner, root.retained_until, &root)
                .await
                .map(|_| ())
        };
        // A lost create acknowledgement grants custody only after the exact root
        // and immutable payload are visible; it never grants send authority.
        let committed = if matches!(
            &write,
            Err(ExecutionError::Busy | ExecutionError::Unavailable | ExecutionError::Uncertain)
        ) {
            self.read_admitted(&root.id, plan, target).await?
        } else {
            self.read(&root.id, plan, target).await?
        };
        match committed {
            Some(committed) => same(committed, &operation, &content_hash),
            // Busy means admitted without a receipt. Root publication contention
            // alone is no proof of admission; keep it a transient 503 instead.
            None => Err(match write {
                Ok(()) | Err(ExecutionError::Busy) => ExecutionError::Uncertain,
                Err(error) => error,
            }),
        }
    }
    async fn read_admitted(
        &self,
        id: &str,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Option<Root>, ExecutionError> {
        crate::readback::bounded(
            || self.read(id, plan, target),
            || ExecutionError::Unavailable,
        )
        .await
    }
    pub async fn payload(&self, root: &Root) -> Result<Payload, ExecutionError> {
        let mut bytes = Vec::with_capacity(root.bytes);
        for index in 0..root.bytes.div_ceil(CHUNK_BYTES) {
            let (chunk, version) = self
                .0
                .read::<Chunk>(&root.chunk_id(index))
                .await?
                .ok_or(ExecutionError::Corrupt)?;
            if version != 1
                || chunk.kind != format!("{KIND}_chunk")
                || chunk.work != root.id
                || chunk.payload_hash != root.payload_hash
                || chunk.index != index
                || chunk.data.len() > CHUNK_BYTES.div_ceil(3) * 4
            {
                return Err(ExecutionError::Corrupt);
            }
            let data = STANDARD
                .decode(&chunk.data)
                .map_err(|_| ExecutionError::Corrupt)?;
            if data.len() != (root.bytes - index * CHUNK_BYTES).min(CHUNK_BYTES) {
                return Err(ExecutionError::Corrupt);
            }
            bytes.extend(data);
        }
        if storage::hash(&bytes) != root.payload_hash {
            return Err(ExecutionError::Corrupt);
        }
        let payload: Payload =
            serde_json::from_slice(&bytes).map_err(|_| ExecutionError::Corrupt)?;
        let content_hash = match &root.answer {
            Some(answer) => answer.content_hash(&payload.message)?,
            None => storage::hash(&storage::bytes(&payload.message)?),
        };
        if content_hash != root.content_hash || payload.capability.expires_at != root.expires_at {
            return Err(ExecutionError::Corrupt);
        }
        Ok(payload)
    }
    pub async fn transition(
        &self,
        root: &mut Root,
        token: &PartitionLease,
        state: State,
    ) -> Result<(), ExecutionError> {
        let mut next = root.clone();
        next.state = state;
        next.version = self
            .0
            .write_effect_fenced(
                &Some(token.clone()),
                &next.id,
                &next.identity.owner,
                next.retained_until,
                &next,
                Some(root.version),
                next.active(),
            )
            .await?;
        *root = next;
        Ok(())
    }
    pub async fn receipt(&self, root: &Root) -> Result<Option<IntentReceipt>, ExecutionError> {
        let stored = match &root.state {
            State::Settled { receipt }
            | State::Unknown {
                receipt: Some(receipt),
            } => receipt,
            State::Unknown { receipt: None } => {
                return Ok(Some(crate::delivery::durable::receipt_for(
                    crate::delivery::intents::DeliveryOutcome::Unknown {
                        reason: "replica_delayed_reply_uncertain".into(),
                    },
                )))
            }
            State::Queued | State::Sending => return Ok(None),
        };
        self.0
            .receipts(&root.operation, &root.identity.owner, root.retained_until)
            .load(stored)
            .await
            .map(Some)
            .map_err(|_| ExecutionError::Unavailable)
    }
}
fn same(root: Root, operation: &str, content_hash: &str) -> Result<Root, ExecutionError> {
    if root.operation != operation || root.content_hash != content_hash {
        return Err(ExecutionError::IdempotencyConflict);
    }
    Ok(root)
}
