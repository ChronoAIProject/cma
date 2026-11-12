//! Persistence evidence accompanies the accounting value through a decision.
use layer2_core::budget::{BudgetError, PlatformPeriodLedger};
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use super::period::period_key;
use super::{decode, ApplyError, DOCUMENT_SCHEMA_VERSION};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PeriodState {
    pub(super) ledger: PlatformPeriodLedger,
}

impl From<PlatformPeriodLedger> for PeriodState {
    fn from(ledger: PlatformPeriodLedger) -> Self {
        Self { ledger }
    }
}

impl PeriodState {
    pub(super) fn document(&self) -> Result<Document, BudgetError> {
        let record =
            mongodb::bson::serialize_to_bson(&self.ledger).map_err(|_| BudgetError::Unavailable)?;
        Ok(doc! {
            "_id": period_key(self.ledger.period_start),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION, "record": record,
        })
    }
}

pub(super) async fn read(
    rows: &Collection<Document>,
    start: u64,
    session: &mut ClientSession,
) -> Result<Option<PeriodState>, ApplyError> {
    rows.find_one(doc! {"_id": period_key(start)})
        .session(session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(|document| decode::<PlatformPeriodLedger>(document).map(PeriodState::from))
        .transpose()
}

pub(super) async fn read_plain(
    rows: &Collection<Document>,
    start: u64,
) -> Result<Option<PeriodState>, BudgetError> {
    rows.find_one(doc! {"_id": period_key(start)})
        .await
        .map_err(|_| BudgetError::Unavailable)?
        .as_ref()
        .map(|document| {
            decode::<PlatformPeriodLedger>(document)
                .map(PeriodState::from)
                .map_err(|_| BudgetError::Unavailable)
        })
        .transpose()
}
