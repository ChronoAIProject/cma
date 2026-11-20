//! Conversion of the preflighted row with separate working and retained custody.

use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope, OwnedWork, SharedOwned,
};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::{Document, RawDocument, RawDocumentBuf};

use super::layout::{BsonLayout, LayoutError};

/// Includes simultaneous conversion copies and the final shared payload owner.
pub(crate) struct ConversionPlan {
    pub(crate) temporary_bytes: usize,
    pub(crate) retained_bytes: usize,
}

/// Each persisted shape supplies its own node bounds and existing validator.
pub(crate) trait RowSchema {
    type Value;
    const INVALID_RAW: &'static str;

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError>;
    fn decode(row: Document) -> Result<Self::Value, SandboxStoreError>;
}

#[derive(Debug)]
pub(crate) enum RowError {
    Refused(LayoutError),
    Domain(OwnedWork<SandboxStoreError>),
}

impl From<LayoutError> for RowError {
    fn from(error: LayoutError) -> Self {
        Self::Refused(error)
    }
}

/// The caller's plan covers its query, raw response and bounded error mapping.
/// Pass a factory: constructing a query/future before this entry evades admission.
/// Driver allocation limits and detached cleanup need their own qualified port.
pub(crate) async fn load<S, F>(
    policy: &IngressOwnershipPolicy,
    read_plan: AllocationPlan,
    initialize: impl FnOnce() -> F,
) -> Result<Option<SharedOwned<S::Value>>, RowError>
where
    S: RowSchema,
    F: std::future::Future<Output = Result<Option<RawDocumentBuf>, SandboxStoreError>>,
{
    let mut read_storage = policy
        .admit(IngressClass::Transformation, read_plan)
        .map_err(|_| LayoutError::Capacity)?;
    let raw = match initialize().await {
        Ok(raw) => raw,
        Err(error) => {
            return Err(RowError::Domain(
                IngressWorkScope::new(read_storage).initialize_owned(|| error),
            ));
        }
    };
    let Some(raw) = raw else {
        return Ok(None);
    };
    // BSON's wire length can be shorter than its Vec backing. Moving the bytes
    // exposes that capacity without copying or changing the validated header.
    let bytes = raw.into_bytes();
    let raw_storage = read_storage
        .split_admitted(
            AllocationPlan::checked(
                "ingress returned raw row",
                &[bytes.capacity(), shared_owner_bytes::<RawDocumentBuf>()],
            )
            .map_err(|_| LayoutError::Capacity)?,
        )
        .map_err(|_| LayoutError::Capacity)?;
    let raw = RawDocumentBuf::from_bytes(bytes).map_err(|_| LayoutError::Structure)?;
    let raw = raw_storage.initialize(|| raw);
    drop(read_storage);
    decode::<S>(&raw, policy).map(Some)
}

pub(crate) fn decode<S: RowSchema>(
    raw: &RawDocument,
    policy: &IngressOwnershipPolicy,
) -> Result<SharedOwned<S::Value>, RowError> {
    let layout = BsonLayout::inspect(raw, policy, "ingress BSON preflight errors")?;
    let plan = S::plan(layout)?;
    let working = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked(
                "ingress row conversion",
                &[
                    plan.temporary_bytes,
                    shared_owner_bytes::<SandboxStoreError>(),
                ],
            )
            .map_err(|_| LayoutError::Capacity)?,
        )
        .map_err(|_| LayoutError::Capacity)?;
    let retained = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("ingress retained row", &[plan.retained_bytes])
                .map_err(|_| LayoutError::Capacity)?,
        )
        .map_err(|_| LayoutError::Capacity)?;
    // Both reservations precede even the scope allocation. Conversion errors
    // keep working custody; successful values release it and keep Data only.
    let working = IngressWorkScope::new(working);
    let result = Document::try_from(raw)
        .map_err(|_| SandboxStoreError::Corrupt(S::INVALID_RAW.into()))
        .and_then(S::decode);
    match result {
        Ok(value) => Ok(retained.initialize(|| value)),
        Err(error) => Err(RowError::Domain(working.initialize_owned(|| error))),
    }
}
