//! Two bounded encrypted snapshot slots. No per-delta history accumulates.
//! Only the inactive slot is changed; the target CAS publishes it atomically.
use super::{
    model::{BODY_CHUNK, MAX_BODY},
    replay_projection::ReplayProjection,
    storage::{bytes, hash, identifier},
    ExecutionError, Journal, Lease, TargetState,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
const KIND: &str = "ordinary_route_projection_v1";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectionReference {
    slot: u8,
    hash: String,
    len: usize,
    versions: Vec<u64>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Chunk {
    kind: String,
    target: String,
    root: String,
    request_hash: String,
    response_id: String,
    projection_hash: String,
    retained_until: i64,
    slot: u8,
    index: usize,
    count: usize,
    data: String,
}
impl Journal {
    pub(super) async fn save_projection(
        &self,
        lease: &mut Lease,
        projection: &ReplayProjection,
    ) -> Result<(), ExecutionError> {
        self.guard(lease).await?;
        projection.validate(lease)?;
        let encoded = bytes(projection)?;
        if encoded.is_empty() || encoded.len() > MAX_BODY {
            return Err(ExecutionError::Invalid);
        }
        let slot = lease.state.projection.as_ref().map_or(0, |v| 1 - v.slot);
        let mut reference = ProjectionReference {
            slot,
            hash: hash(&encoded),
            len: encoded.len(),
            versions: Vec::new(),
        };
        for (index, data) in encoded.chunks(BODY_CHUNK).enumerate() {
            self.refresh_projection_lease(lease).await?;
            let state = &lease.state;
            let id = chunk_id(state, slot, index);
            let previous = self.read::<Chunk>(&id).await?;
            if let Some((chunk, _)) = &previous {
                validate_chunk(chunk, state, slot, index, &projection.response_id)?;
            }
            self.guard(lease).await?;
            let chunk = Chunk {
                kind: KIND.into(),
                target: state.id.clone(),
                root: state.root.clone(),
                request_hash: state.request_hash.clone(),
                response_id: projection.response_id.clone(),
                projection_hash: reference.hash.clone(),
                retained_until: state.retained_until,
                slot,
                index,
                count: encoded.len().div_ceil(BODY_CHUNK),
                data: STANDARD.encode(data),
            };
            reference.versions.push(
                self.write(
                    &id,
                    &state.owner,
                    state.retained_until,
                    &chunk,
                    previous.map(|(_, version)| version),
                )
                .await?,
            );
        }
        self.guard(lease).await?;
        let mut next = lease.state.clone();
        next.projection = Some(reference);
        self.commit(lease, next).await
    }
    pub(super) async fn load_projection(
        &self,
        lease: &Lease,
    ) -> Result<Option<ReplayProjection>, ExecutionError> {
        self.guard(lease).await?;
        let state = &lease.state;
        let Some(reference) = &state.projection else {
            return Ok(None);
        };
        if reference.slot > 1
            || !(1..=MAX_BODY).contains(&reference.len)
            || reference.versions.len() != reference.len.div_ceil(BODY_CHUNK)
        {
            return Err(ExecutionError::Corrupt);
        }
        let response_id = match &state.phase {
            super::Phase::Accepted { response_id, .. } => response_id,
            _ => return Err(ExecutionError::Invalid),
        };
        let mut encoded = Vec::with_capacity(reference.len);
        for (index, expected_version) in reference.versions.iter().enumerate() {
            let (chunk, version) = self
                .read::<Chunk>(&chunk_id(state, reference.slot, index))
                .await?
                .ok_or(ExecutionError::Corrupt)?;
            validate_chunk(&chunk, state, reference.slot, index, response_id)?;
            if version != *expected_version
                || chunk.projection_hash != reference.hash
                || chunk.count != reference.versions.len()
            {
                return Err(ExecutionError::Corrupt);
            }
            let decoded = STANDARD
                .decode(chunk.data)
                .map_err(|_| ExecutionError::Corrupt)?;
            if decoded.len() != (reference.len - index * BODY_CHUNK).min(BODY_CHUNK) {
                return Err(ExecutionError::Corrupt);
            }
            encoded.extend(decoded);
        }
        if hash(&encoded) != reference.hash {
            return Err(ExecutionError::Corrupt);
        }
        let projection: ReplayProjection =
            serde_json::from_slice(&encoded).map_err(|_| ExecutionError::Corrupt)?;
        projection.validate(lease)?;
        self.guard(lease).await?;
        Ok(Some(projection))
    }
    async fn refresh_projection_lease(&self, lease: &mut Lease) -> Result<(), ExecutionError> {
        if lease.state.ownership.until - (self.clock)() <= 10 {
            self.renew(lease).await
        } else {
            self.guard(lease).await
        }
    }
}
fn validate_chunk(
    chunk: &Chunk,
    state: &TargetState,
    slot: u8,
    index: usize,
    response_id: &str,
) -> Result<(), ExecutionError> {
    if chunk.kind != KIND
        || chunk.target != state.id
        || chunk.root != state.root
        || chunk.request_hash != state.request_hash
        || chunk.retained_until != state.retained_until
        || chunk.response_id != response_id
        || chunk.slot != slot
        || chunk.index != index
    {
        return Err(ExecutionError::Corrupt);
    }
    Ok(())
}
fn chunk_id(state: &TargetState, slot: u8, index: usize) -> String {
    identifier(&(
        KIND,
        &state.id,
        &state.root,
        &state.request_hash,
        slot,
        index,
    ))
}
