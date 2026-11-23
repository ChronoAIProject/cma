use mongodb::bson::{doc, Document};

use super::ScanError;

const LIMIT: i64 = 1024 * 1024;

pub(crate) struct ProofBudget {
    remaining: i64,
}

impl Default for ProofBudget {
    fn default() -> Self {
        Self { remaining: LIMIT }
    }
}

impl ProofBudget {
    pub(crate) fn remaining(&self) -> i64 {
        self.remaining
    }

    pub(crate) fn charge(&mut self, bytes: i64) -> Result<(), ScanError> {
        if bytes <= 0 {
            return Err(ScanError::Invalid);
        }
        if bytes > self.remaining {
            self.remaining = 0;
            return Err(ScanError::ProofBudget);
        }
        self.remaining -= bytes;
        Ok(())
    }

    pub(super) fn charge_row(&mut self, row: &Document) -> Result<(), ScanError> {
        self.charge(i64::from(
            row.get_i32("proof_bytes").map_err(|_| ScanError::Invalid)?,
        ))
    }

    // Return a size marker for a record outside the cap. Do not transfer it
    // or confuse an oversized record with an absent optional record.
    pub(super) fn projection(&self, row_cap: i64) -> Document {
        doc! {
            "_id": 0,
            "proof_bytes": {"$bsonSize": "$$ROOT"},
            "evidence": {"$cond": [
                {"$lte": [{"$bsonSize": "$$ROOT"}, self.remaining.min(row_cap)]},
                "$$ROOT", "$$REMOVE",
            ]},
        }
    }

    pub(super) fn take_row(&mut self, row: Document) -> Result<Document, ScanError> {
        self.charge_row(&row)?;
        row.get_document("evidence").cloned().map_err(|_| {
            self.remaining = 0;
            ScanError::ProofBudget
        })
    }
}
