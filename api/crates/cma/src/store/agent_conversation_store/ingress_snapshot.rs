//! Allocation-owned snapshot reads using the same row and original predicate.

use layer2_core::sandbox::ingress_ownership::{
    AllocationPlan, IngressClass, IngressOwnershipPolicy, SharedOwned,
};
use layer2_core::sandbox::store::{AgentMessageSnapshot, SandboxStoreError};
use layer2_core::AgentId;
use mongodb::bson::{doc, Document, RawDocumentBuf};

use super::{checkpoint, ingress, read_agent, MongoSandboxStore};

pub(super) async fn load(
    store: &MongoSandboxStore,
    id: &AgentId,
    policy: &IngressOwnershipPolicy,
) -> Result<Option<SharedOwned<AgentMessageSnapshot>>, SandboxStoreError> {
    let mut read_storage = ingress::read_allowance(policy, id.as_str().len())?;
    let raw = store
        .agents
        .clone_with_type::<RawDocumentBuf>()
        .find_one(doc! { "_id": id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    // find_one has completed and dropped its cursor/response. Keep custody of
    // the returned raw buffer while returning the no-longer-live read copies.
    let raw_storage = read_storage
        .split(
            AllocationPlan::new(
                "conversation returned raw snapshot",
                &[raw.as_bytes().len(), size_of::<RawDocumentBuf>()],
            )
            .map_err(|_| ingress::unavailable())?,
        )
        .map_err(|_| ingress::unavailable())?;
    drop(read_storage);
    let result = decode(&raw, policy);
    drop(raw);
    drop(raw_storage);
    result.map(Some)
}

fn decode(
    raw: &mongodb::bson::RawDocument,
    policy: &IngressOwnershipPolicy,
) -> Result<SharedOwned<AgentMessageSnapshot>, SandboxStoreError> {
    let layout = ingress::SnapshotLayout::inspect(raw)?;
    let working = ingress::reserve(
        policy,
        IngressClass::Transformation,
        "conversation snapshot decode",
        &[layout.decode_bytes()?],
    )?;
    let retained = ingress::reserve(
        policy,
        IngressClass::Data,
        "conversation retained snapshot",
        &[layout.retained_bytes()?],
    )?;
    let row = Document::try_from(raw)
        .map_err(|_| SandboxStoreError::Corrupt("invalid raw conversation snapshot".into()))?;
    let result = retained.try_initialize(|| {
        Ok(AgentMessageSnapshot {
            agent: read_agent(&row)?,
            checkpoint: checkpoint::read(&row)?,
        })
    });
    drop(row);
    drop(working);
    result
}
