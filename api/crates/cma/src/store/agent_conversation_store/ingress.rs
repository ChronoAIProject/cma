//! Borrowed BSON preflight; wire length and requested allocation are distinct.

use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    ReservationBundle,
};
use layer2_core::sandbox::store::{AgentMessageSnapshot, SandboxStoreError};
use mongodb::bson::{Bson, Document, RawBsonRef, RawDocument};

pub(super) const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_DEPTH: usize = 64;

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

#[derive(Debug, Default)]
pub(super) struct SnapshotLayout {
    pub(super) wire_bytes: usize,
    strings: usize,
    bson_slots: usize,
    domain_slots: usize,
    documents: usize,
}

impl SnapshotLayout {
    pub(super) fn inspect(
        raw: &RawDocument,
        policy: &IngressOwnershipPolicy,
    ) -> Result<Self, SnapshotRefusal> {
        let mut layout = Self {
            wire_bytes: raw.as_bytes().len(),
            ..Self::default()
        };
        if layout.wire_bytes > MAX_DOCUMENT_BYTES {
            return Err(corrupt());
        }
        // BSON 3.1 RawIter can copy a failing key and format numeric diagnostics.
        // Its longest raw-parser message is under 128 bytes (two usize values);
        // 1024 covers its formatting/reallocation/copy overlap. Two wire lengths
        // cover key replacement while the original key allocation is still live.
        let _diagnostics = reserve(
            policy,
            IngressClass::Transformation,
            "conversation BSON preflight errors",
            &[mul(layout.wire_bytes, 2)?, 1024],
        )?;
        layout.document(raw, 0)?;
        Ok(layout)
    }

    fn document(&mut self, raw: &RawDocument, depth: usize) -> Result<(), SnapshotRefusal> {
        if depth > MAX_DEPTH {
            return Err(corrupt());
        }
        self.documents = add(self.documents, 1)?;
        let mut entries = 0;
        for element in raw.iter() {
            let (key, value) = element.map_err(|_| corrupt())?;
            self.strings = add(self.strings, key.len())?;
            self.value(value, depth + 1)?;
            entries = add(entries, 1)?;
        }
        // IndexMap's dense buckets and hash table may both grow. Six slots per
        // entry cover old/new power-of-two capacity, including tiny maps.
        self.bson_slots = add(self.bson_slots, mul(entries, 6)?)?;
        // Some documents become domain BTreeMaps rather than inline structs.
        // Count each pair as a node too; string bytes alone miss map storage.
        self.domain_slots = add(self.domain_slots, entries)?;
        Ok(())
    }

    fn value(&mut self, value: RawBsonRef<'_>, depth: usize) -> Result<(), SnapshotRefusal> {
        if depth > MAX_DEPTH {
            return Err(corrupt());
        }
        match value {
            RawBsonRef::Document(document) => self.document(document, depth)?,
            RawBsonRef::Array(array) => {
                let mut entries = 0;
                for value in array {
                    self.value(value.map_err(|_| corrupt())?, depth + 1)?;
                    entries = add(entries, 1)?;
                }
                // Vec growth can retain both buffers; four is its minimum
                // nonzero capacity for these element layouts.
                let slots = if entries == 0 {
                    0
                } else {
                    mul(entries, 3)?.max(4)
                };
                self.bson_slots = add(self.bson_slots, slots)?;
                self.domain_slots = add(self.domain_slots, slots)?;
            }
            RawBsonRef::String(value)
            | RawBsonRef::JavaScriptCode(value)
            | RawBsonRef::Symbol(value) => self.strings = add(self.strings, value.len())?,
            RawBsonRef::Binary(value) => self.strings = add(self.strings, value.bytes.len())?,
            RawBsonRef::RegularExpression(value) => {
                self.strings = add(self.strings, add(value.pattern.len(), value.options.len())?)?;
            }
            RawBsonRef::JavaScriptCodeWithScope(value) => {
                self.strings = add(self.strings, value.code.len())?;
                self.document(value.scope, depth)?;
            }
            RawBsonRef::DbPointer(_) => return Err(corrupt()),
            RawBsonRef::Double(_)
            | RawBsonRef::Boolean(_)
            | RawBsonRef::Null
            | RawBsonRef::Int32(_)
            | RawBsonRef::Int64(_)
            | RawBsonRef::Timestamp(_)
            | RawBsonRef::ObjectId(_)
            | RawBsonRef::DateTime(_)
            | RawBsonRef::Decimal128(_)
            | RawBsonRef::Undefined
            | RawBsonRef::MaxKey
            | RawBsonRef::MinKey => {}
        }
        Ok(())
    }

    fn bson_bytes(&self) -> Result<usize, SnapshotRefusal> {
        // A dense bucket carries the key, value and hash. The separate table
        // carries an index plus control byte; pointer padding covers alignment.
        let slot = size_of::<(String, Bson)>() + 3 * size_of::<usize>();
        add(
            self.strings,
            add(mul(self.bson_slots, slot)?, mul(self.documents, 64)?)?,
        )
    }

    pub(super) fn retained_bytes(&self) -> Result<usize, SnapshotRefusal> {
        // Agent is an upper layout bound for persisted sequence members and
        // boxed domain records. This deliberately overcounts inline structs;
        // accepted rows never rely on their encoded byte count as heap size.
        let nodes = add(self.domain_slots, self.documents)?;
        add(
            shared_owner_bytes::<AgentMessageSnapshot>(),
            add(
                self.strings,
                mul(nodes, size_of::<layer2_core::sandbox::Agent>())?,
            )?,
        )
    }

    pub(super) fn decode_bytes(&self) -> Result<usize, SnapshotRefusal> {
        // Materialized Document, the cloned record/checkpoint consumed by BSON
        // deserialization, and serde's buffered content may overlap. Checkpoint
        // validation streams into a byte limit and retains no JSON buffer.
        add(mul(self.bson_bytes()?, 3)?, mul(self.retained_bytes()?, 2)?)
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
