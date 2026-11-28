//! Transactional conversion and accounting behind the period storage boundary.
use layer2_core::budget::BudgetError;
use mongodb::ClientSession;

use super::counters::{self, Change, Counter};
use super::period::{fence_settings, read_period, save_period};
use super::period_storage::PeriodState;
use super::{domain, ApplyError, MongoSandboxStore};

#[derive(Clone, Copy)]
pub(super) struct Charge {
    pub(super) start: u64,
    pub(super) shard: Option<u16>,
}

/// Conversion shares the admission's settings fence and transaction. Older
/// writers either conflict with the root replacement or reject its marker.
pub(super) async fn activate(
    store: &MongoSandboxStore,
    state: &mut PeriodState,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    if state.counters.is_some() || !store.platform_counter_shards {
        return Ok(());
    }
    let counters: Vec<_> = (0..counters::COUNT)
        .map(|shard| Counter {
            period_start: state.ledger.period_start,
            shard,
            spent: if shard == 0 { state.ledger.spent } else { 0 },
            reserved: if shard == 0 { state.ledger.reserved } else { 0 },
            revision: 0,
        })
        .collect();
    let documents = counters
        .iter()
        .map(Counter::document)
        .collect::<Result<Vec<_>, _>>()
        .map_err(domain)?;
    store
        .model_budget
        .insert_many(documents)
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    state.counters = Some(counters);
    save_period(store, state, session).await?;
    state.ledger.revision += 1;
    tracing::debug!(
        event = "platform_counter_conversion",
        "prepared bounded counter layout in admission transaction"
    );
    Ok(())
}

/// The caller already checked the exact sum under the settings write fence.
/// Only this partition changes unless the successful admission enters warning
/// or adopts updated period metadata.
pub(super) async fn reserve(
    store: &MongoSandboxStore,
    state: &PeriodState,
    owner: &layer2_core::SubjectId,
    liability: u64,
    metadata_changed: bool,
    session: &mut ClientSession,
) -> Result<Charge, ApplyError> {
    let shard = if let Some(counters) = &state.counters {
        let shard = counters::partition(owner);
        let mut counter = counters[usize::from(shard)].clone();
        Change {
            retained: liability,
            ..Default::default()
        }
        .apply(&mut counter.spent, &mut counter.reserved)
        .map_err(domain)?;
        counters::save(&store.model_budget, &counter, session).await?;
        Some(shard)
    } else {
        None
    };
    if shard.is_none() || metadata_changed {
        save_period(store, state, session).await?;
    }
    Ok(Charge {
        start: state.ledger.period_start,
        shard,
    })
}

pub(super) async fn mirror(
    store: &MongoSandboxStore,
    start: Option<u64>,
    shard: Option<u16>,
    change: Change,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let Some(start) = start else {
        return if shard.is_none() {
            Ok(())
        } else {
            Err(domain(BudgetError::Unavailable))
        };
    };
    if let Some(shard) = shard {
        let counter = counters::read_one(&store.model_budget, start, shard, session).await?;
        return change_counter(store, counter, change, session).await;
    }
    // Legacy reservations retain their original period and move together to
    // partition zero. They need no record backfill and cannot double-settle.
    let mut state = read_period(&store.model_budget, start, session)
        .await?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    if let Some(counters) = state.counters {
        change_counter(store, counters[0].clone(), change, session).await
    } else {
        change
            .apply(&mut state.ledger.spent, &mut state.ledger.reserved)
            .map_err(domain)?;
        save_period(store, &state, session).await
    }
}

async fn change_counter(
    store: &MongoSandboxStore,
    mut counter: Counter,
    change: Change,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    // A release or ordinary settlement cannot increase committed spend. Late
    // overage can, so it shares admission's settings fence to prevent skew.
    if change.increases_total() {
        fence_settings(
            &store.budget_settings,
            "platform_counter_increase_fence",
            session,
        )
        .await?;
    }
    change
        .apply(&mut counter.spent, &mut counter.reserved)
        .map_err(domain)?;
    counters::save(&store.model_budget, &counter, session).await
}
