//! Borrowed BSON preflight; wire length and requested allocation are distinct.

use crate::store::ingress_reads::layout::{BsonLayout, LayoutError};
use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    ReservationBundle,
};
use layer2_core::sandbox::store::{AgentMessageSnapshot, SandboxStoreError};
use mongodb::bson::Document;
#[cfg(test)]
use mongodb::bson::RawDocument;

pub(super) use crate::store::ingress_reads::layout::MAX_DOCUMENT_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SnapshotRefusal {
    Capacity,
    Structure,
}

impl SnapshotRefusal {
    // Refusal can occur before any allocation is admitted. Preserve its category
    // without constructing a heap diagnostic outside a work owner.
    pub(super) fn into_error(self) -> SandboxStoreError {
        match self {
            Self::Capacity => SandboxStoreError::Unavailable(String::new()),
            Self::Structure => SandboxStoreError::Corrupt(String::new()),
        }
    }
}

impl From<LayoutError> for SnapshotRefusal {
    fn from(error: LayoutError) -> Self {
        match error {
            LayoutError::Capacity => Self::Capacity,
            LayoutError::Structure => Self::Structure,
        }
    }
}

fn corrupt() -> SnapshotRefusal {
    SnapshotRefusal::Structure
}

fn add(a: usize, b: usize) -> Result<usize, SnapshotRefusal> {
    a.checked_add(b).ok_or_else(corrupt)
}

fn mul(a: usize, b: usize) -> Result<usize, SnapshotRefusal> {
    a.checked_mul(b).ok_or_else(corrupt)
}

pub(super) fn reserve(
    policy: &IngressOwnershipPolicy,
    class: IngressClass,
    phase: &'static str,
    allocations: &[usize],
) -> Result<ReservationBundle, SnapshotRefusal> {
    let plan =
        AllocationPlan::checked(phase, allocations).map_err(|_| SnapshotRefusal::Capacity)?;
    policy
        .admit(class, plan)
        .map_err(|_| SnapshotRefusal::Capacity)
}

#[derive(Debug)]
pub(super) struct SnapshotLayout {
    #[cfg(test)]
    pub(super) wire_bytes: usize,
    raw: BsonLayout,
}

impl SnapshotLayout {
    #[cfg(test)]
    pub(super) fn inspect(
        raw: &RawDocument,
        policy: &IngressOwnershipPolicy,
    ) -> Result<Self, SnapshotRefusal> {
        let raw = BsonLayout::inspect(raw, policy, "conversation BSON preflight errors")
            .map_err(SnapshotRefusal::from)?;
        Ok(Self::from_raw(raw))
    }

    pub(super) fn from_raw(raw: BsonLayout) -> Self {
        Self {
            #[cfg(test)]
            wire_bytes: raw.wire_bytes,
            raw,
        }
    }

    pub(super) fn retained_bytes(&self) -> Result<usize, SnapshotRefusal> {
        // Agent bounds this schema's sequence members and boxed domain records.
        self.raw
            .domain_bytes(
                size_of::<layer2_core::sandbox::Agent>(),
                shared_owner_bytes::<AgentMessageSnapshot>(),
            )
            .map_err(|_| corrupt())
    }

    pub(super) fn decode_bytes(&self) -> Result<usize, SnapshotRefusal> {
        // Document, cloned record/checkpoint, serde buffering and domain output.
        add(
            mul(self.raw.bson_bytes().map_err(|_| corrupt())?, 3)?,
            mul(self.retained_bytes()?, 2)?,
        )
    }
}

pub(super) fn read_allowance(
    policy: &IngressOwnershipPolicy,
    id_bytes: usize,
) -> Result<ReservationBundle, SnapshotRefusal> {
    // find_one requests a single row. The pinned driver's response backing,
    // cursor row copy and RawDocumentBuf deserialization can overlap. Allow a
    // complete maximum BSON document for each, plus command/response envelopes.
    reserve(
        policy,
        IngressClass::Transformation,
        "conversation raw snapshot",
        &[
            3 * MAX_DOCUMENT_BYTES,
            64 * 1024,
            id_bytes,
            size_of::<Document>(),
            shared_owner_bytes::<SandboxStoreError>(),
        ],
    )
}
