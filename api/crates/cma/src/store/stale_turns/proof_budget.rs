use std::{
    future::Future,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
};

use mongodb::{
    bson::{doc, Document},
    options::ReadConcern,
    ClientSession, Collection,
};

use super::ScanError;

const LIMIT: i64 = 1024 * 1024;

tokio::task_local! {
    static WRITER_BUDGET: ProofBudget;
}

#[derive(Clone)]
pub(crate) struct ProofBudget {
    remaining: Arc<AtomicI64>,
}

impl Default for ProofBudget {
    fn default() -> Self {
        Self {
            remaining: Arc::new(AtomicI64::new(LIMIT)),
        }
    }
}

impl ProofBudget {
    pub(crate) fn remaining(&self) -> i64 {
        self.remaining.load(Ordering::Relaxed)
    }

    pub(crate) fn charge(&self, bytes: i64) -> Result<(), ScanError> {
        if bytes <= 0 {
            return Err(ScanError::Invalid);
        }
        let previous = self
            .remaining
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                Some(remaining.saturating_sub(bytes).max(0))
            })
            .unwrap_or_else(|remaining| remaining);
        (previous >= bytes)
            .then_some(())
            .ok_or(ScanError::ProofBudget)
    }

    pub(super) fn charge_row(&self, row: &Document) -> Result<(), ScanError> {
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
                {"$lte": [{"$bsonSize": "$$ROOT"}, self.remaining().min(row_cap)]},
                "$$ROOT", "$$REMOVE",
            ]},
        }
    }

    pub(super) fn take_row(&self, row: Document) -> Result<Document, ScanError> {
        self.charge_row(&row)?;
        row.get_document("evidence").cloned().map_err(|_| {
            self.remaining.store(0, Ordering::Relaxed);
            ScanError::ProofBudget
        })
    }

    pub(super) async fn with_writer<F: Future>(&self, work: F) -> F::Output {
        WRITER_BUDGET.scope(self.clone(), work).await
    }
}

/// Exact writer point reads share the stale-turn limit when publication runs
/// in that scope. A size marker refuses the row before BSON payload transfer.
pub(in crate::store) async fn find_with_proof_budget(
    collection: &Collection<Document>,
    filter: Document,
    session: &mut ClientSession,
    snapshot: bool,
) -> mongodb::error::Result<Option<Document>> {
    let budget = WRITER_BUDGET.try_with(Clone::clone).ok();
    let mut query = collection.find_one(filter);
    if snapshot {
        query = query.read_concern(ReadConcern::snapshot());
    }
    if let Some(budget) = &budget {
        query = query.projection(budget.projection(LIMIT));
    }
    let row = query.session(session).await?;
    match (budget, row) {
        (Some(budget), Some(row)) => budget
            .take_row(row)
            .map(Some)
            .map_err(mongodb::error::Error::custom),
        (_, row) => Ok(row),
    }
}
