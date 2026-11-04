use super::{
    model::*,
    storage::{bytes, hash, identifier},
    Journal,
};
use crate::ingress::routing_store::{AdmittedPlan, StoredTarget};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Chunk {
    kind: String,
    target: String,
    root: String,
    request_hash: String,
    body_hash: String,
    retained_until: i64,
    index: usize,
    count: usize,
    data: String,
}
impl Journal {
    pub(super) async fn freeze(
        &self,
        lease: &mut Lease,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        body: &FrozenTurn,
    ) -> Result<(), ExecutionError> {
        self.guard(lease).await?;
        if lease.state.phase != Phase::Pending || lease.state.body.is_some() {
            return Err(ExecutionError::Invalid);
        }
        body.validate(plan, target, &self.keys, (self.clock)())?;
        match &lease.state.binding_preparation {
            Some(preparation) => preparation.require_body_generation(body.binding_generation)?,
            None if body.binding_generation.is_some() || plan.prepares_binding(target) => {
                return Err(ExecutionError::Invalid)
            }
            None => {}
        }
        let encoded = bytes(body)?;
        if encoded.len() > MAX_BODY {
            return Err(ExecutionError::Invalid);
        }
        self.bind_tool_reply(plan, target).await?;
        let digest = hash(&encoded);
        let mut reference = BodyReference {
            hash: digest.clone(),
            len: encoded.len(),
            chunks: Vec::new(),
        };
        for (index, data) in encoded.chunks(BODY_CHUNK).enumerate() {
            self.guard(lease).await?;
            let id = chunk_id(&lease.state, &digest, index);
            let chunk = Chunk {
                kind: BODY_KIND.into(),
                target: lease.state.id.clone(),
                root: lease.state.root.clone(),
                request_hash: lease.state.request_hash.clone(),
                body_hash: digest.clone(),
                retained_until: lease.state.retained_until,
                index,
                count: encoded.len().div_ceil(BODY_CHUNK),
                data: STANDARD.encode(data),
            };
            self.write(
                &id,
                &lease.state.owner,
                lease.state.retained_until,
                &chunk,
                None,
            )
            .await?;
            reference.chunks.push(id);
        }
        self.guard(lease).await?;
        let mut next = lease.state.clone();
        next.body = Some(reference);
        next.phase = Phase::Prepared;
        self.commit(lease, next).await
    }

    pub(super) async fn body(&self, lease: &Lease) -> Result<FrozenTurn, ExecutionError> {
        self.guard(lease).await?;
        let body = self.retained_body(&lease.state).await?;
        self.guard(lease).await?;
        Ok(body)
    }
    /// Read immutable frozen input without acquiring/mutating the original
    /// response lease. Callers must independently bind the retained checkpoint
    /// to their admitted plan and current effect authority.
    pub(super) async fn retained_body(
        &self,
        state: &TargetState,
    ) -> Result<FrozenTurn, ExecutionError> {
        let reference = state.body.as_ref().ok_or(ExecutionError::Corrupt)?;
        if !(1..=MAX_BODY).contains(&reference.len)
            || reference.chunks.len() != reference.len.div_ceil(BODY_CHUNK)
        {
            return Err(ExecutionError::Corrupt);
        }
        let mut result = Vec::with_capacity(reference.len);
        for (index, id) in reference.chunks.iter().enumerate() {
            if *id != chunk_id(state, &reference.hash, index) {
                return Err(ExecutionError::Corrupt);
            }
            let (chunk, version) = self
                .read::<Chunk>(id)
                .await?
                .ok_or(ExecutionError::Corrupt)?;
            if version != 1
                || chunk.kind != BODY_KIND
                || chunk.target != state.id
                || chunk.root != state.root
                || chunk.request_hash != state.request_hash
                || chunk.body_hash != reference.hash
                || chunk.retained_until != state.retained_until
                || chunk.index != index
                || chunk.count != reference.chunks.len()
            {
                return Err(ExecutionError::Corrupt);
            }
            let decoded = STANDARD
                .decode(chunk.data)
                .map_err(|_| ExecutionError::Corrupt)?;
            let expected = (reference.len - index * BODY_CHUNK).min(BODY_CHUNK);
            if decoded.len() != expected {
                return Err(ExecutionError::Corrupt);
            }
            result.extend(decoded);
        }
        if hash(&result) != reference.hash {
            return Err(ExecutionError::Corrupt);
        }
        let body: FrozenTurn =
            serde_json::from_slice(&result).map_err(|_| ExecutionError::Corrupt)?;
        match &state.binding_preparation {
            Some(preparation) => preparation.require_body_generation(body.binding_generation)?,
            None if body.binding_generation.is_some() => return Err(ExecutionError::Corrupt),
            None => {}
        }
        Ok(body)
    }
}
fn chunk_id(state: &TargetState, hash: &str, index: usize) -> String {
    identifier(&(&state.id, &state.root, &state.request_hash, hash, index))
}
