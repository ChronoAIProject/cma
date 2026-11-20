//! Retained page storage; the source keeps raw batches owned during conversion.
use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AdmittedError, AllocationPlan, IngressClass,
    IngressOwnershipPolicy, IngressWorkScope, OwnedWork, SharedOwned,
};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::{Document, RawDocument};

use super::{
    layout::{BsonLayout, LayoutError},
    row::{self, ConversionPlan, RowError, RowSchema},
};

pub(crate) const MAX_ROWS: u32 = 256;
pub(crate) const MAX_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct DocumentSchema;

impl RowSchema for DocumentSchema {
    type Value = Document;
    const INVALID_RAW: &'static str = "transaction page BSON is invalid";

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        let bson = layout.bson_bytes()?;
        Ok(ConversionPlan {
            temporary_bytes: bson.checked_add(1024).ok_or(LayoutError::Structure)?,
            retained_bytes: bson
                .checked_add(shared_owner_bytes::<Document>())
                .ok_or(LayoutError::Structure)?,
        })
    }

    fn decode(row: Document) -> Result<Document, SandboxStoreError> {
        Ok(row)
    }
}

#[derive(Debug)]
pub(crate) enum PageError {
    Row(RowError),
    Invalid(&'static str),
}

impl From<RowError> for PageError {
    fn from(error: RowError) -> Self {
        Self::Row(error)
    }
}
impl From<LayoutError> for PageError {
    fn from(error: LayoutError) -> Self {
        Self::Row(error.into())
    }
}

pub(crate) struct Page {
    rows: Vec<SharedOwned<Document>>,
    storage: IngressWorkScope,
    bytes: usize,
    limit: usize,
}

impl Page {
    /// Reserve the final vector once, before its allocation or the first query.
    pub(crate) fn new(policy: &IngressOwnershipPolicy, limit: u32) -> Result<Self, AdmittedError> {
        if !(1..=MAX_ROWS).contains(&limit) {
            return Err(
                layer2_core::sandbox::ingress_ownership::AdmissionRefusal::InvalidPlan {
                    phase: "transaction page limit",
                }
                .into(),
            );
        }
        let storage = IngressWorkScope::new(policy.admit(
            IngressClass::Data,
            AllocationPlan::checked("retained transaction page", &[
                size_of::<SharedOwned<Document>>().checked_mul(limit as usize)
                    .ok_or(layer2_core::sandbox::ingress_ownership::AdmissionRefusal::InvalidPlan {
                        phase: "transaction page vector",
                    })?,
                shared_owner_bytes::<Vec<SharedOwned<Document>>>(),
            ])?,
        )?);
        Ok(Self {
            rows: Vec::with_capacity(limit as usize),
            storage,
            bytes: 0,
            limit: limit as usize,
        })
    }

    pub(crate) fn push(
        &mut self,
        raw: &RawDocument,
        policy: &IngressOwnershipPolicy,
    ) -> Result<(), PageError> {
        let layout = BsonLayout::inspect(raw, policy, "transaction page serialization")?;
        let scratch = layout
            .bson_bytes()?
            .checked_add(layout.wire_bytes)
            .and_then(|bytes| bytes.checked_mul(3))
            .and_then(|bytes| bytes.checked_add(1024))
            .ok_or(LayoutError::Structure)?;
        let _serialization = policy
            .admit(
                IngressClass::Transformation,
                AllocationPlan::checked("transaction page canonical BSON", &[scratch])
                    .map_err(|_| LayoutError::Capacity)?,
            )
            .map_err(|_| LayoutError::Capacity)?;
        let row = row::decode::<DocumentSchema>(raw, policy)?;
        // Match legacy accounting after BSON materialization, including repeated
        // keys. Wire length alone is not the canonical Document serialization.
        let bytes = mongodb::bson::serialize_to_vec(&*row)
            .map_err(|_| PageError::Invalid("transaction page BSON is invalid"))?
            .len();
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|bytes| *bytes <= MAX_BYTES)
            .ok_or(PageError::Invalid(
                "transaction page exceeds its byte budget",
            ))?;
        if self.rows.len() >= self.limit {
            return Err(PageError::Invalid(
                "transaction page exceeds its row budget",
            ));
        }
        self.rows.push(row);
        Ok(())
    }

    pub(crate) fn finish(self) -> OwnedWork<Vec<SharedOwned<Document>>> {
        self.storage.initialize_owned(|| self.rows)
    }
}
