//! Preserve image release order and serialize independent Desktop sizing selections.
use super::*;
use crate::store::{
    commit_transaction, is_confirmed_body_retry, start_transaction,
    transaction_terminal::GuardedTerminal as _, CommitOutcome,
};
use mongodb::ClientSession;
use std::time::Duration;

const ATTEMPTS: usize = 5;

enum SelectionError {
    Driver(mongodb::error::Error),
    Authority(WarmPoolError),
}

impl From<mongodb::error::Error> for SelectionError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}
impl From<WarmPoolError> for SelectionError {
    fn from(error: WarmPoolError) -> Self {
        Self::Authority(error)
    }
}

pub(super) async fn activate(
    store: &MongoNativeWarmPools,
    candidate: &PoolTarget,
) -> Result<(), WarmPoolError> {
    if candidate.machine_plan_activated_at.is_some() {
        return Ok(());
    }
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(1 << attempt)).await;
        }
        let mut session = store
            .database
            .client()
            .start_session()
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_confirmed_body_retry(&error) {
                continue;
            }
            return Err(WarmPoolError::Unavailable);
        }
        match select_once(store, candidate, &mut session).await {
            Ok(false) => {
                session
                    .abort_guarded()
                    .await
                    .map_err(|_| WarmPoolError::Unavailable)?;
                return Ok(());
            }
            Ok(true) => (),
            Err(error) => {
                if let Err(abort) = session.abort_guarded().await {
                    tracing::warn!(%abort, "Desktop plan selection abort was not acknowledged");
                }
                match error {
                    SelectionError::Driver(error) if is_confirmed_body_retry(&error) => continue,
                    SelectionError::Driver(error) => {
                        tracing::warn!(%error, "Desktop plan selection transaction failed");
                        return Err(WarmPoolError::Unavailable);
                    }
                    SelectionError::Authority(error) => return Err(error),
                }
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Failed(error) => {
                tracing::warn!(%error, "Desktop plan selection commit failed");
                return Err(WarmPoolError::Unavailable);
            }
            CommitOutcome::Unknown(error) => {
                tracing::warn!(%error, "Desktop plan selection commit is uncertain; checking durable plan");
                // A committed selection is immutable. A different winning
                // candidate still fails the caller's latest-release check.
                let exact = store.find(&candidate.id).await?.is_some_and(|row| {
                    row.machine
                        && row.plan == candidate.plan
                        && !row.retiring
                        && !row.retired
                        && row.activated_at.is_some()
                        && row.machine_plan_activated_at.is_some()
                });
                return if exact {
                    Ok(())
                } else {
                    Err(WarmPoolError::Unavailable)
                };
            }
        }
    }
    Err(WarmPoolError::Unavailable)
}

async fn select_once(
    store: &MongoNativeWarmPools,
    candidate: &PoolTarget,
    session: &mut ClientSession,
) -> Result<bool, SelectionError> {
    let mut scope = release_scope(store, true);
    scope.insert("activated_at", doc! {"$type":"date"});
    // The earliest released row remains as a tombstone. Contenders modifying
    // different sizing rows must share its transaction write fence.
    let anchor = store
        .targets
        .find_one(scope)
        .sort(doc! {"activated_at":1,"_id":1})
        .session(&mut *session)
        .await?;
    let anchor = match anchor {
        Some(row) => row,
        None => store
            .targets
            .find_one(release_scope(store, true))
            .sort(doc! {"_id":1})
            .session(&mut *session)
            .await?
            .ok_or(WarmPoolError::Conflict)?,
    };
    let mut anchor_filter = release_scope(store, true);
    anchor_filter.insert("_id", &anchor.id);
    let fenced = store
        .targets
        .update_one(
            anchor_filter,
            doc! {"$inc":{"machine_plan_selection_fence":1_i64}},
        )
        .session(&mut *session)
        .await?;
    if fenced.matched_count != 1 {
        return Err(WarmPoolError::Conflict.into());
    }

    let mut exact = release_scope(store, true);
    exact.extend(doc! {"_id":&candidate.id,"retiring":{"$ne":true},"retired":{"$ne":true}});
    let row = store
        .targets
        .find_one(exact.clone())
        .session(&mut *session)
        .await?
        .ok_or(WarmPoolError::Conflict)?;
    if row.machine_plan_activated_at.is_none() {
        let mut image_scope = release_scope(store, true);
        image_scope.extend(doc! {"plan.spec.image.uri": &row.plan.spec.image.uri,
        "activated_at": {"$type":"date"}});
        let mut selected = image_scope.clone();
        selected.insert("machine_plan_activated_at", doc! {"$type":"date"});
        let previously_selected = store
            .targets
            .find_one(selected)
            .sort(doc! {"machine_plan_activated_at": -1})
            .session(&mut *session)
            .await?
            .and_then(|row| row.machine_plan_activated_at);
        if row.activated_at.is_none() || previously_selected.is_none() {
            let previous = store
                .targets
                .find_one(image_scope)
                .sort(doc! {"activated_at":1})
                .session(&mut *session)
                .await?
                .and_then(|row| row.activated_at);
            let mut fields = doc! {"machine_plan_activated_at":"$$NOW"};
            if let Some(at) = previously_selected {
                let next = DateTime::from_millis(
                    at.timestamp_millis()
                        .checked_add(1)
                        .ok_or(WarmPoolError::Unavailable)?,
                );
                fields.insert("machine_plan_activated_at", doc! {"$max":["$$NOW",next]});
            }
            if row.activated_at.is_none() {
                match previous {
                    Some(at) => {
                        fields.insert("activated_at", at);
                    }
                    None => {
                        fields.insert("activated_at", "$$NOW");
                    }
                }
            }
            exact.insert("machine_plan_activated_at", mongodb::bson::Bson::Null);
            let written = store
                .targets
                .update_one(exact, vec![doc! {"$set":fields}])
                .session(&mut *session)
                .await?;
            if written.matched_count != 1 {
                return Err(WarmPoolError::Conflict.into());
            }
            return Ok(true);
        }
    }
    Ok(false)
}
