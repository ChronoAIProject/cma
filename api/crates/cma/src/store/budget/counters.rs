//! Bounded model counters. Admission is serialized by the existing settings
//! fence; ordinary settlements need only the counter recorded on their hold.
use layer2_core::budget::BudgetError;
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{decode, domain, ApplyError, DOCUMENT_SCHEMA_VERSION};

pub(super) const COUNT: u16 = 16;
pub(super) const LAYOUT: i32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Counter {
    pub(super) period_start: u64,
    pub(super) shard: u16,
    pub(super) spent: u64,
    pub(super) reserved: u64,
    pub(super) revision: u64,
}

/// Zero contains all obligations admitted before conversion. New obligations
/// use a stable owner partition so restart and process count do not move holds.
pub(super) fn partition(owner: &layer2_core::SubjectId) -> u16 {
    1 + u16::from(Sha256::digest(owner.as_str().as_bytes())[0]) % (COUNT - 1)
}

pub(super) fn key(start: u64, shard: u16) -> String {
    format!("platform-counter:{start}:{shard}")
}

pub(super) fn keys(start: u64) -> Vec<String> {
    (0..COUNT).map(|shard| key(start, shard)).collect()
}

impl Counter {
    pub(super) fn document(&self) -> Result<Document, ApplyError> {
        let record =
            mongodb::bson::serialize_to_bson(self).map_err(|_| domain(BudgetError::Unavailable))?;
        Ok(doc! {"_id": key(self.period_start, self.shard),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION, "record": record})
    }

    pub(super) fn decode(document: &Document, start: u64) -> Result<Self, ApplyError> {
        let counter: Self = decode(document)?;
        if counter.period_start != start
            || counter.shard >= COUNT
            || document.get_str("_id").ok() != Some(key(start, counter.shard).as_str())
            || [counter.spent, counter.reserved, counter.revision]
                .into_iter()
                .any(|value| value > i64::MAX as u64)
        {
            return Err(domain(BudgetError::Unavailable));
        }
        Ok(counter)
    }
}

pub(super) async fn read_one(
    rows: &Collection<Document>,
    start: u64,
    shard: u16,
    session: &mut ClientSession,
) -> Result<Counter, ApplyError> {
    let document = crate::store::stale_turns::find_with_proof_budget(
        rows,
        doc! {"_id": key(start, shard)},
        session,
        false,
    )
    .await
    .map_err(ApplyError::Driver)?
    .ok_or_else(|| domain(BudgetError::Unavailable))?;
    Counter::decode(&document, start)
}

pub(super) async fn save(
    rows: &Collection<Document>,
    counter: &Counter,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let mut next = counter.clone();
    next.revision = counter
        .revision
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let result = rows
        .replace_one(
            doc! {"_id": key(counter.period_start, counter.shard),
            "record.revision": counter.revision as i64},
            next.document()?,
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if result.matched_count != 1 {
        return Err(domain(BudgetError::Unavailable));
    }
    Ok(())
}

/// Closure decisions must fence every component that contributed to their
/// sum. An actual write detects a release after the evaluator's snapshot.
pub(super) async fn fence(
    rows: &Collection<Document>,
    counters: &[Counter],
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    let filters: Vec<_> = counters
        .iter()
        .map(|counter| {
            doc! {
                "_id": key(counter.period_start, counter.shard),
                "record.revision": counter.revision as i64,
            }
        })
        .collect();
    let result = rows
        .update_many(
            doc! {"$or": filters},
            doc! {"$inc": {"platform_transition_fence": 1_i64}},
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    Ok(result.matched_count == u64::from(COUNT))
}

/// The same checked change applies to an original period or a distributed
/// counter. Late incurred usage remains accounted even when it exceeds a cap.
#[derive(Clone, Copy, Default)]
pub(super) struct Change {
    pub(super) retained: u64,
    pub(super) released: u64,
    pub(super) debit: u64,
}

impl Change {
    pub(super) fn apply(self, spent: &mut u64, reserved: &mut u64) -> Result<(), BudgetError> {
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

    pub(super) fn increases_total(self) -> bool {
        u128::from(self.retained) + u128::from(self.debit) > u128::from(self.released)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounting_changes_preserve_balances_on_invalid_arithmetic() {
        for (spent, reserved, change) in [
            (
                0,
                1,
                Change {
                    released: 2,
                    ..Default::default()
                },
            ),
            (
                0,
                1,
                Change {
                    retained: i64::MAX as u64,
                    ..Default::default()
                },
            ),
            (
                i64::MAX as u64,
                1,
                Change {
                    released: 1,
                    debit: 1,
                    ..Default::default()
                },
            ),
        ] {
            let mut actual = (spent, reserved);
            assert_eq!(
                change.apply(&mut actual.0, &mut actual.1),
                Err(BudgetError::Unavailable)
            );
            assert_eq!(actual, (spent, reserved));
        }
        let mut actual = (0, 10);
        Change {
            retained: 5,
            released: 15,
            debit: 12,
        }
        .apply(&mut actual.0, &mut actual.1)
        .unwrap();
        assert_eq!(actual, (12, 0));
        assert!(Change {
            retained: 5,
            released: 15,
            debit: 12
        }
        .increases_total());
        assert!(!Change {
            released: 15,
            debit: 15,
            ..Default::default()
        }
        .increases_total());
    }
}
