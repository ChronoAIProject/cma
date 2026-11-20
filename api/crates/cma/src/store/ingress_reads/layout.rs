//! Borrowed raw structure and container sizing, independent of the stored schema.
use layer2_core::sandbox::ingress_ownership::{
    AllocationPlan, IngressClass, IngressOwnershipPolicy,
};
use mongodb::bson::{Bson, RawBsonRef, RawDocument};

pub(crate) const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayoutError {
    Capacity,
    Structure,
}

fn corrupt() -> LayoutError {
    LayoutError::Structure
}
fn add(a: usize, b: usize) -> Result<usize, LayoutError> {
    a.checked_add(b).ok_or_else(corrupt)
}
fn mul(a: usize, b: usize) -> Result<usize, LayoutError> {
    a.checked_mul(b).ok_or_else(corrupt)
}
fn reserve(
    policy: &IngressOwnershipPolicy,
    class: IngressClass,
    phase: &'static str,
    allocations: &[usize],
) -> Result<layer2_core::sandbox::ingress_ownership::ReservationBundle, LayoutError> {
    policy
        .admit(
            class,
            AllocationPlan::checked(phase, allocations).map_err(|_| LayoutError::Capacity)?,
        )
        .map_err(|_| LayoutError::Capacity)
}

#[derive(Debug, Default)]
pub(crate) struct BsonLayout {
    pub(crate) wire_bytes: usize,
    strings: usize,
    bson_slots: usize,
    domain_slots: usize,
    documents: usize,
}

impl BsonLayout {
    pub(crate) fn inspect(
        raw: &RawDocument,
        policy: &IngressOwnershipPolicy,
        phase: &'static str,
    ) -> Result<Self, LayoutError> {
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
            phase,
            &[mul(layout.wire_bytes, 2)?, 1024],
        )?;
        layout.document(raw, 0)?;
        Ok(layout)
    }

    fn document(&mut self, raw: &RawDocument, depth: usize) -> Result<(), LayoutError> {
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

    fn value(&mut self, value: RawBsonRef<'_>, depth: usize) -> Result<(), LayoutError> {
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

    pub(crate) fn bson_bytes(&self) -> Result<usize, LayoutError> {
        // A dense bucket carries the key, value and hash. The separate table
        // carries an index plus control byte; pointer padding covers alignment.
        let slot = size_of::<(String, Bson)>() + 3 * size_of::<usize>();
        add(
            self.strings,
            add(mul(self.bson_slots, slot)?, mul(self.documents, 64)?)?,
        )
    }

    /// The caller supplies its schema's largest allocated node and owner layout.
    /// BSON structure alone cannot establish a domain type's allocation bound.
    pub(crate) fn domain_bytes(
        &self,
        maximum_node_bytes: usize,
        owner_bytes: usize,
    ) -> Result<usize, LayoutError> {
        let nodes = add(self.domain_slots, self.documents)?;
        add(
            owner_bytes,
            add(self.strings, mul(nodes, maximum_node_bytes)?)?,
        )
    }
}
