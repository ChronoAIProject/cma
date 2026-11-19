//! Allocation-owned snapshot reads using the same row and original predicate.

use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope, OwnedWork, SharedOwned,
};
use layer2_core::sandbox::store::{AgentMessageSnapshot, SandboxStoreError};
use layer2_core::AgentId;
use mongodb::bson::{doc, Document, RawDocumentBuf};

use super::ingress::SnapshotRefusal;
use super::{checkpoint, ingress, read_agent, MongoSandboxStore};

#[derive(Debug)]
pub(super) enum SnapshotError {
    Refused(SnapshotRefusal),
    Owned(OwnedWork<SandboxStoreError>),
}

impl From<SnapshotRefusal> for SnapshotError {
    fn from(refusal: SnapshotRefusal) -> Self {
        Self::Refused(refusal)
    }
}

pub(super) async fn load(
    store: &MongoSandboxStore,
    id: &AgentId,
    policy: &IngressOwnershipPolicy,
) -> Result<Option<SharedOwned<AgentMessageSnapshot>>, OwnedWork<SandboxStoreError>> {
    match load_inner(store, id, policy).await {
        Ok(snapshot) => Ok(snapshot),
        Err(SnapshotError::Owned(error)) => Err(error),
        Err(SnapshotError::Refused(refusal)) => {
            Err(IngressWorkScope::inherit_owned(async { refusal.into_error() }).await)
        }
    }
}

async fn load_inner(
    store: &MongoSandboxStore,
    id: &AgentId,
    policy: &IngressOwnershipPolicy,
) -> Result<Option<SharedOwned<AgentMessageSnapshot>>, SnapshotError> {
    let mut read_storage = ingress::read_allowance(policy, id.as_str().len())?;
    let raw = match store
        .agents
        .clone_with_type::<RawDocumentBuf>()
        .find_one(doc! { "_id": id.as_str() })
        .await
    {
        Ok(raw) => raw,
        Err(error) => {
            let scope = IngressWorkScope::new(read_storage);
            drop(error);
            // The read plan covers this bounded diagnostic and its owner. Do not
            // format a driver payload into an independently escaping String.
            return Err(SnapshotError::Owned(scope.initialize_owned(|| {
                SandboxStoreError::Unavailable("conversation snapshot read failed".into())
            })));
        }
    };
    let Some(raw) = raw else {
        return Ok(None);
    };
    // find_one has completed and dropped its cursor/response. Keep custody of
    // the returned raw buffer while returning the no-longer-live read copies.
    let raw_storage = read_storage
        .split_admitted(
            AllocationPlan::checked(
                "conversation returned raw snapshot",
                &[raw.as_bytes().len(), shared_owner_bytes::<RawDocumentBuf>()],
            )
            .map_err(|_| SnapshotRefusal::Capacity)?,
        )
        .map_err(|_| SnapshotRefusal::Capacity)?;
    let raw = raw_storage.initialize(|| raw);
    drop(read_storage);
    let result = decode(&raw, policy);
    drop(raw);
    result.map(Some)
}

pub(super) fn decode(
    raw: &mongodb::bson::RawDocument,
    policy: &IngressOwnershipPolicy,
) -> Result<SharedOwned<AgentMessageSnapshot>, SnapshotError> {
    let layout = ingress::SnapshotLayout::inspect(raw, policy)?;
    let working = IngressWorkScope::new(ingress::reserve(
        policy,
        IngressClass::Transformation,
        "conversation snapshot decode",
        &[
            layout.decode_bytes()?,
            shared_owner_bytes::<SandboxStoreError>(),
        ],
    )?);
    let retained = ingress::reserve(
        policy,
        IngressClass::Data,
        "conversation retained snapshot",
        &[layout.retained_bytes()?],
    )?;
    let result = (|| {
        let row = Document::try_from(raw)
            .map_err(|_| SandboxStoreError::Corrupt("invalid raw conversation snapshot".into()))?;
        Ok(AgentMessageSnapshot {
            agent: read_agent(&row)?,
            checkpoint: checkpoint::read(&row)?,
        })
    })();
    match result {
        Ok(snapshot) => Ok(retained.initialize(|| snapshot)),
        Err(error) => Err(SnapshotError::Owned(working.initialize_owned(|| error))),
    }
}
