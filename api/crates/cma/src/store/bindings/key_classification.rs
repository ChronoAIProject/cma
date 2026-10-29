//! A unique-document linearizable barrier precedes any negative history lookup.
use super::*;
use mongodb::{
    options::{ReadConcern, ReadPreference},
    ClientSession,
};
use std::time::Duration;

pub(super) async fn contains(
    store: &MongoSandboxStore,
    hash: &str,
) -> Result<bool, SandboxStoreError> {
    tokio::time::timeout(Duration::from_millis(900), async {
        match current_or_barrier(store, hash).await? {
            CurrentOrBarrier::Known => Ok(true),
            CurrentOrBarrier::History(mut barrier) => history(store, hash, &mut barrier).await,
        }
    })
    .await
    .map_err(|_| {
        SandboxStoreError::Unavailable("worker key classification deadline exceeded".into())
    })?
}

enum CurrentOrBarrier {
    Known,
    History(Box<ClientSession>),
}

async fn current_or_barrier(
    store: &MongoSandboxStore,
    hash: &str,
) -> Result<CurrentOrBarrier, SandboxStoreError> {
    // New logical sessions have no operationTime. Reusing a session could add
    // afterClusterTime to a linearizable read, which Mongo does not support.
    let mut session = store
        .client
        .start_session()
        .causal_consistency(true)
        .await
        .map_err(driver)?;
    if session.operation_time().is_some() {
        return Err(unavailable_barrier());
    }
    // record.key_sha256 has a UNIQUE index. A non-unique $or/history filter must
    // not be used as this barrier: Mongo's linearizable guarantee is restricted
    // to a filter that uniquely identifies one document, including its absence.
    let current = store
        .bindings
        .records
        .find_one(doc! {"record.key_sha256": hash})
        .read_concern(ReadConcern::linearizable())
        .selection_criteria(ReadPreference::Primary)
        .max_time(Duration::from_millis(750))
        .projection(doc! {"_id": 1})
        .session(&mut session)
        .await
        .map_err(driver)?;
    if current.is_some() {
        return Ok(CurrentOrBarrier::Known);
    }
    if session.operation_time().is_none() {
        return Err(unavailable_barrier());
    }
    Ok(CurrentOrBarrier::History(Box::new(session)))
}

async fn history(
    store: &MongoSandboxStore,
    hash: &str,
    barrier: &mut ClientSession,
) -> Result<bool, SandboxStoreError> {
    if barrier.operation_time().is_none() {
        return Err(unavailable_barrier());
    }
    // mongodb 3.9.1 carries the first response's operationTime in this same
    // causal session as readConcern.afterClusterTime. Majority history reads
    // cannot precede the linearizable barrier or acknowledged prior rotations.
    // This path performs no transaction and creates no record or extra ledger.
    Ok(store
        .bindings
        .records
        .find_one(doc! {"key_history": hash})
        .read_concern(ReadConcern::majority())
        .selection_criteria(ReadPreference::Primary)
        .max_time(Duration::from_millis(750))
        .projection(doc! {"_id": 1})
        .session(barrier)
        .await
        .map_err(driver)?
        .is_some())
}

fn unavailable_barrier() -> SandboxStoreError {
    SandboxStoreError::Unavailable("worker key read barrier is unavailable".into())
}

#[cfg(test)]
mod tests;
