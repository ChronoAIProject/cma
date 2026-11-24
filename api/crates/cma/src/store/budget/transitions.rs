//! A closure transition is valid only while both its policy and accounting
//! evidence are current. The settings write prevents snapshot write skew.
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::budget::BudgetError;
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use super::period::period_key;
use super::period_storage::PeriodState;
use super::{ApplyError, DOCUMENT_SCHEMA_VERSION};
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
};

/// `None` means the period did not exist in the deciding snapshot. A concurrent
/// creator or revision change invalidates the whole transition, including entry.
pub(super) async fn persist(
    settings: &Collection<Document>,
    rows: &Collection<Document>,
    settings_revision: u64,
    expected: Option<u64>,
    state: &PeriodState,
) -> Result<bool, BudgetError> {
    let mut session = rows
        .client()
        .start_session()
        .await
        .map_err(|_| BudgetError::Unavailable)?;
    start_transaction(&mut session)
        .await
        .map_err(|_| BudgetError::Unavailable)?;
    let result = write(
        settings,
        rows,
        settings_revision,
        expected,
        state,
        &mut session,
    )
    .await;
    match result {
        Ok(true) => match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(true),
            CommitOutcome::Retry(_) => Ok(false),
            CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => Err(BudgetError::Unavailable),
        },
        result => {
            if let Err(error) = session.abort_guarded().await {
                tracing::warn!(event = "platform_spend_transition_abort", error = %error,
                    "could not confirm closure transition abort");
            }
            match result {
                Ok(false) => Ok(false),
                Err(ApplyError::Driver(error))
                    if is_transient(&error) || is_duplicate_key(&error) =>
                {
                    Ok(false)
                }
                _ => Err(BudgetError::Unavailable),
            }
        }
    }
}

async fn write(
    settings: &Collection<Document>,
    rows: &Collection<Document>,
    settings_revision: u64,
    expected: Option<u64>,
    state: &PeriodState,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    let signed = |value| i64::try_from(value).map_err(|_| super::domain(BudgetError::Unavailable));
    let fenced = settings
        .update_one(
            doc! {"_id": "platform", "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(settings_revision)?},
            doc! {"$inc": {"platform_spend_transition_fence": 1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if fenced.matched_count != 1 {
        return Ok(false);
    }
    if let Some(counters) = &state.counters {
        if !super::counters::fence(rows, counters, session).await? {
            return Ok(false);
        }
    }
    let ledger = &state.ledger;
    let mut next = state.clone();
    next.ledger.revision = ledger
        .revision
        .checked_add(1)
        .filter(|revision| *revision <= i64::MAX as u64)
        .ok_or_else(|| super::domain(BudgetError::Unavailable))?;
    let document = next.document().map_err(super::domain)?;
    match expected {
        Some(revision) => Ok(rows
            .replace_one(
                doc! {"_id": period_key(ledger.period_start), "record.revision": signed(revision)?},
                document,
            )
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .matched_count
            == 1),
        None => {
            rows.insert_one(document)
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?;
            Ok(true)
        }
    }
}
