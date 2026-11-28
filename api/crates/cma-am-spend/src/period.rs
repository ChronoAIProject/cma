//! Platform period records and nontransactional read models.

use futures_util::TryStreamExt;
use layer2_core::budget::{BudgetError, PlatformPeriodLedger};
use mongodb::bson::{doc, Document};
use mongodb::Collection;

use crate::counters::{self, Counter};
use crate::{period_key, DOCUMENT_SCHEMA_VERSION};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeriodState {
    pub ledger: PlatformPeriodLedger,
    pub counters: Option<Vec<Counter>>,
}

impl From<PlatformPeriodLedger> for PeriodState {
    fn from(ledger: PlatformPeriodLedger) -> Self {
        Self {
            ledger,
            counters: None,
        }
    }
}

impl PeriodState {
    pub fn document(&self) -> Result<Document, BudgetError> {
        let mut ledger = self.ledger.clone();
        if self.counters.is_some() {
            ledger.spent = 0;
            ledger.reserved = 0;
        }
        let mut record =
            mongodb::bson::serialize_to_document(&ledger).map_err(|_| BudgetError::Unavailable)?;
        if self.counters.is_some() {
            // An old strict reader must refuse this row rather than treating
            // zero root balances as permission to spend the same budget again.
            record.insert("counter_layout", counters::LAYOUT);
        }
        Ok(doc! {"_id": period_key(ledger.period_start),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION, "record": record})
    }

    /// Root-only decoding is sufficient for persisted STOP authority. Decisions
    /// involving amounts must also load and validate every declared partition.
    pub fn root(document: &Document) -> Result<Self, BudgetError> {
        let mut document = document.clone();
        let record = document
            .get_document_mut("record")
            .map_err(|_| BudgetError::Unavailable)?;
        let distributed = match record.remove("counter_layout") {
            None => false,
            Some(mongodb::bson::Bson::Int32(counters::LAYOUT)) => true,
            _ => return Err(BudgetError::Unavailable),
        };
        let ledger: PlatformPeriodLedger = crate::decode(&document)?;
        if document.get_str("_id").ok() != Some(period_key(ledger.period_start).as_str())
            || (distributed && (ledger.spent != 0 || ledger.reserved != 0))
        {
            return Err(BudgetError::Unavailable);
        }
        Ok(Self {
            ledger,
            counters: distributed.then(Vec::new),
        })
    }

    pub fn sum(&mut self, documents: Vec<Document>) -> Result<(), BudgetError> {
        let mut counters = documents
            .iter()
            .map(|document| Counter::decode(document, self.ledger.period_start))
            .collect::<Result<Vec<_>, _>>()?;
        counters.sort_by_key(|counter| counter.shard);
        if counters.len() != usize::from(counters::COUNT)
            || counters
                .iter()
                .enumerate()
                .any(|(index, counter)| usize::from(counter.shard) != index)
        {
            return Err(BudgetError::Unavailable);
        }
        let sum = |field: fn(&Counter) -> u64| {
            counters.iter().try_fold(0_u64, |total, counter| {
                total
                    .checked_add(field(counter))
                    .filter(|value| *value <= i64::MAX as u64)
                    .ok_or(BudgetError::Unavailable)
            })
        };
        self.ledger.spent = sum(|counter| counter.spent)?;
        self.ledger.reserved = sum(|counter| counter.reserved)?;
        self.counters = Some(counters);
        Ok(())
    }
}

pub fn batch_keys(start: u64) -> Vec<String> {
    let mut keys = counters::keys(start);
    keys.push(period_key(start));
    keys
}

pub fn from_batch(
    start: u64,
    mut documents: Vec<Document>,
) -> Result<Option<PeriodState>, BudgetError> {
    let key = period_key(start);
    let Some(index) = documents
        .iter()
        .position(|document| document.get_str("_id").ok() == Some(key.as_str()))
    else {
        return if documents.is_empty() {
            Ok(None)
        } else {
            Err(BudgetError::Unavailable)
        };
    };
    let mut state = PeriodState::root(&documents.swap_remove(index))?;
    if state.counters.is_some() {
        state.sum(documents)?;
    } else if !documents.is_empty() {
        return Err(BudgetError::Unavailable);
    }
    Ok(Some(state))
}

pub async fn read_plain(
    rows: &Collection<Document>,
    start: u64,
    batched: bool,
) -> Result<Option<PeriodState>, BudgetError> {
    if batched {
        let documents = rows
            .find(doc! {"_id": {"$in": batch_keys(start)}})
            .await
            .map_err(|_| BudgetError::Unavailable)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|_| BudgetError::Unavailable)?;
        return from_batch(start, documents);
    }
    let Some(document) = rows
        .find_one(doc! {"_id": period_key(start)})
        .await
        .map_err(|_| BudgetError::Unavailable)?
    else {
        return Ok(None);
    };
    let mut state = PeriodState::root(&document)?;
    if state.counters.is_some() {
        let documents = rows
            .find(doc! {"_id": {"$in": counters::keys(start)}})
            .await
            .map_err(|_| BudgetError::Unavailable)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|_| BudgetError::Unavailable)?;
        state.sum(documents)?;
    }
    Ok(Some(state))
}
