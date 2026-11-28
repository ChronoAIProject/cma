//! Caller-owned transaction operations for spend counters.
use layer2_core::budget::BudgetError;
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use super::{domain, ApplyError};
pub(super) use cma_am_spend::counters::{key, keys, partition, Change, Counter, COUNT};

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
    Counter::decode(&document, start).map_err(domain)
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
            next.document().map_err(domain)?,
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
