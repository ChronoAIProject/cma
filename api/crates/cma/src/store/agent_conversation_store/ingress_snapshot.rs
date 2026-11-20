//! Allocation-owned snapshot reads using the same row and original predicate.

use crate::store::ingress_reads::{
    layout::{BsonLayout, LayoutError},
    row::{self, ConversionPlan, RowError, RowSchema},
};
use layer2_core::sandbox::ingress_ownership::{
    AdmittedError, AdmittedOperation, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope, OwnedWork, SharedOwned,
};
use layer2_core::sandbox::store::{AgentMessageSnapshot, SandboxStoreError};
use layer2_core::AgentId;
use mongodb::bson::{doc, Document, RawDocumentBuf};

use super::ingress::SnapshotRefusal;
use super::{checkpoint, ingress, read_agent, MongoSandboxStore};

pub(super) type SnapshotOperation<'a> =
    AdmittedOperation<'a, Result<Option<SharedOwned<AgentMessageSnapshot>>, SandboxStoreError>>;

pub(super) fn operation<'a>(
    store: &'a MongoSandboxStore,
    id: &'a AgentId,
    policy: &'a IngressOwnershipPolicy,
) -> Result<SnapshotOperation<'a>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked("conversation snapshot operation", &[])?,
        move |scope| async move {
            Ok(match load_inner(store, id, policy).await {
                Ok(snapshot) => scope.initialize_owned(|| Ok(snapshot)),
                Err(SnapshotError::Owned(error)) => error.map(Err),
                Err(SnapshotError::Refused(refusal)) => {
                    scope.initialize_owned(|| Err(refusal.into_error()))
                }
            })
        },
    )
}

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
    row::load::<SnapshotSchema, _>(policy, ingress::read_plan(id.as_str().len())?, || async {
        store
            .agents
            .clone_with_type::<RawDocumentBuf>()
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|_| SandboxStoreError::Unavailable("conversation snapshot read failed".into()))
    })
    .await
    .map_err(snapshot_error)
}

#[cfg(test)]
pub(super) fn decode(
    raw: &mongodb::bson::RawDocument,
    policy: &IngressOwnershipPolicy,
) -> Result<SharedOwned<AgentMessageSnapshot>, SnapshotError> {
    row::decode::<SnapshotSchema>(raw, policy).map_err(snapshot_error)
}

fn snapshot_error(error: RowError) -> SnapshotError {
    match error {
        RowError::Refused(error) => SnapshotError::Refused(error.into()),
        RowError::Domain(error) => SnapshotError::Owned(error),
    }
}

struct SnapshotSchema;

impl RowSchema for SnapshotSchema {
    type Value = AgentMessageSnapshot;
    const INVALID_RAW: &'static str = "invalid raw conversation snapshot";

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        let layout = ingress::SnapshotLayout::from_raw(layout);
        Ok(ConversionPlan {
            temporary_bytes: layout.decode_bytes().map_err(|_| LayoutError::Structure)?,
            retained_bytes: layout
                .retained_bytes()
                .map_err(|_| LayoutError::Structure)?,
        })
    }

    fn decode(row: Document) -> Result<Self::Value, SandboxStoreError> {
        Ok(AgentMessageSnapshot {
            agent: read_agent(&row)?,
            checkpoint: checkpoint::read(&row)?,
        })
    }
}
