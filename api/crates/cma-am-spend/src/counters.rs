//! MongoDB representation and arithmetic for bounded spend counters.

use layer2_core::budget::BudgetError;
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::DOCUMENT_SCHEMA_VERSION;

pub const COUNT: u16 = 16;
pub const LAYOUT: i32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counter {
    pub period_start: u64,
    pub shard: u16,
    pub spent: u64,
    pub reserved: u64,
    pub revision: u64,
}

/// Zero contains all obligations admitted before conversion. New obligations
/// use a stable owner partition so restart and process count do not move holds.
pub fn partition(owner: &layer2_core::SubjectId) -> u16 {
    1 + u16::from(Sha256::digest(owner.as_str().as_bytes())[0]) % (COUNT - 1)
}

pub fn key(start: u64, shard: u16) -> String {
    format!("platform-counter:{start}:{shard}")
}

pub fn keys(start: u64) -> Vec<String> {
    (0..COUNT).map(|shard| key(start, shard)).collect()
}

impl Counter {
    pub fn document(&self) -> Result<Document, BudgetError> {
        let record =
            mongodb::bson::serialize_to_bson(self).map_err(|_| BudgetError::Unavailable)?;
        Ok(doc! {"_id": key(self.period_start, self.shard),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION, "record": record})
    }

    pub fn decode(document: &Document, start: u64) -> Result<Self, BudgetError> {
        let counter: Self = crate::decode(document)?;
        if counter.period_start != start
            || counter.shard >= COUNT
            || document.get_str("_id").ok() != Some(key(start, counter.shard).as_str())
            || [counter.spent, counter.reserved, counter.revision]
                .into_iter()
                .any(|value| value > i64::MAX as u64)
        {
            return Err(BudgetError::Unavailable);
        }
        Ok(counter)
    }
}

/// The same checked change applies to an original period or a distributed
/// counter. Late incurred usage remains accounted even when it exceeds a cap.
#[derive(Clone, Copy, Default)]
pub struct Change {
    pub retained: u64,
    pub released: u64,
    pub debit: u64,
}

impl Change {
    pub fn apply(self, spent: &mut u64, reserved: &mut u64) -> Result<(), BudgetError> {
        let next_reserved = reserved
            .checked_add(self.retained)
            .filter(|value| *value <= i64::MAX as u64)
            .and_then(|value| value.checked_sub(self.released))
            .ok_or(BudgetError::Unavailable)?;
        let next_spent = spent
            .checked_add(self.debit)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or(BudgetError::Unavailable)?;
        *spent = next_spent;
        *reserved = next_reserved;
        Ok(())
    }

    pub fn increases_total(self) -> bool {
        u128::from(self.retained) + u128::from(self.debit) > u128::from(self.released)
    }
}
