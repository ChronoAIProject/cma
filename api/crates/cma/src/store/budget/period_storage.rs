//! Platform period reads governed by the caller's MongoDB transaction.
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use super::period::period_key;
use super::{counters, domain, ApplyError};
use cma_am_spend::period::{batch_keys, from_batch};
pub(super) use cma_am_spend::period::{read_plain, PeriodState};

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
    .map_err(domain)
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
        state.sum(documents).map_err(domain)?;
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
    from_batch(start, documents).map_err(domain)
}
