//! A period value and the exact persistence evidence that produced it.
use futures_util::TryStreamExt;
use layer2_core::budget::{BudgetError, PlatformPeriodLedger};
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use super::counters::{self, Counter};
use super::period::period_key;
use super::{decode, domain, ApplyError, DOCUMENT_SCHEMA_VERSION};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PeriodState {
    pub(super) ledger: PlatformPeriodLedger,
    pub(super) counters: Option<Vec<Counter>>,
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
    pub(super) fn document(&self) -> Result<Document, BudgetError> {
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
    pub(super) fn root(document: &Document) -> Result<Self, ApplyError> {
        let mut document = document.clone();
        let record = document
            .get_document_mut("record")
            .map_err(|_| domain(BudgetError::Unavailable))?;
        let distributed = match record.remove("counter_layout") {
            None => false,
            Some(mongodb::bson::Bson::Int32(counters::LAYOUT)) => true,
            _ => return Err(domain(BudgetError::Unavailable)),
        };
        let ledger: PlatformPeriodLedger = decode(&document)?;
        if document.get_str("_id").ok() != Some(period_key(ledger.period_start).as_str())
            || (distributed && (ledger.spent != 0 || ledger.reserved != 0))
        {
            return Err(domain(BudgetError::Unavailable));
        }
        Ok(Self {
            ledger,
            counters: distributed.then(Vec::new),
        })
    }

    fn sum(&mut self, documents: Vec<Document>) -> Result<(), ApplyError> {
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
            return Err(domain(BudgetError::Unavailable));
        }
        let sum = |field: fn(&Counter) -> u64| {
            counters.iter().try_fold(0_u64, |total, counter| {
                total
                    .checked_add(field(counter))
                    .filter(|value| *value <= i64::MAX as u64)
                    .ok_or_else(|| domain(BudgetError::Unavailable))
            })
        };
        self.ledger.spent = sum(|counter| counter.spent)?;
        self.ledger.reserved = sum(|counter| counter.reserved)?;
        self.counters = Some(counters);
        Ok(())
    }
}

pub(super) async fn read_root(
    rows: &Collection<Document>,
    start: u64,
    session: &mut ClientSession,
) -> Result<Option<PeriodState>, ApplyError> {
    crate::store::stale_turns::find_with_proof_budget(
        rows,
        doc! {"_id": period_key(start)},
        session,
        false,
    )
    .await
    .map_err(ApplyError::Driver)?
    .as_ref()
    .map(PeriodState::root)
    .transpose()
}

pub(super) async fn read(
    rows: &Collection<Document>,
    start: u64,
    session: &mut ClientSession,
) -> Result<Option<PeriodState>, ApplyError> {
    let Some(mut state) = read_root(rows, start, session).await? else {
        return Ok(None);
    };
    if state.counters.is_some() {
        let mut cursor = rows
            .find(doc! {"_id": {"$in": counters::keys(start)}})
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        let mut documents = Vec::new();
        while let Some(document) = cursor.next(&mut *session).await {
            documents.push(document.map_err(ApplyError::Driver)?);
        }
        state.sum(documents)?;
    }
    Ok(Some(state))
}

fn batch_keys(start: u64) -> Vec<String> {
    let mut keys = counters::keys(start);
    keys.push(period_key(start));
    keys
}

fn from_batch(start: u64, mut documents: Vec<Document>) -> Result<Option<PeriodState>, ApplyError> {
    let key = period_key(start);
    let Some(index) = documents
        .iter()
        .position(|document| document.get_str("_id").ok() == Some(key.as_str()))
    else {
        return if documents.is_empty() {
            Ok(None)
        } else {
            Err(domain(BudgetError::Unavailable))
        };
    };
    let mut state = PeriodState::root(&documents.swap_remove(index))?;
    if state.counters.is_some() {
        state.sum(documents)?;
    } else if !documents.is_empty() {
        return Err(domain(BudgetError::Unavailable));
    }
    Ok(Some(state))
}

/// The enabled process reads the root and its fixed key set in one round trip.
/// The returned root remains the authority for whether that layout exists.
pub(super) async fn read_batched(
    rows: &Collection<Document>,
    start: u64,
    session: &mut ClientSession,
) -> Result<Option<PeriodState>, ApplyError> {
    let mut cursor = rows
        .find(doc! {"_id": {"$in": batch_keys(start)}})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let mut documents = Vec::new();
    while let Some(document) = cursor.next(&mut *session).await {
        documents.push(document.map_err(ApplyError::Driver)?);
    }
    from_batch(start, documents)
}

pub(super) async fn read_plain(
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
        return from_batch(start, documents).map_err(|_| BudgetError::Unavailable);
    }
    let Some(document) = rows
        .find_one(doc! {"_id": period_key(start)})
        .await
        .map_err(|_| BudgetError::Unavailable)?
    else {
        return Ok(None);
    };
    let mut state = PeriodState::root(&document).map_err(|_| BudgetError::Unavailable)?;
    if state.counters.is_some() {
        let documents = rows
            .find(doc! {"_id": {"$in": counters::keys(start)}})
            .await
            .map_err(|_| BudgetError::Unavailable)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|_| BudgetError::Unavailable)?;
        state.sum(documents).map_err(|_| BudgetError::Unavailable)?;
    }
    Ok(Some(state))
}
