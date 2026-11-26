//! Preserve image release order and serialize independent Desktop sizing selections.
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(super) async fn activate(
    store: &MongoNativeWarmPools,
    candidate: &PoolTarget,
) -> Result<(), WarmPoolError> {
    if candidate.machine_plan_activated_at.is_some() {
        return Ok(());
    }
    let mut session = store
        .database
        .client()
        .start_session()
        .await
        .map_err(|_| WarmPoolError::Unavailable)?;
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .map_err(|_| WarmPoolError::Unavailable)?;
    let mut scope = release_scope(store, true);
    scope.insert("activated_at", doc! {"$type":"date"});
    // The earliest released row remains as a tombstone. Contenders modifying
    // different sizing rows must share its transaction write fence.
    let anchor = store
        .targets
        .find_one(scope)
        .sort(doc! {"activated_at":1,"_id":1})
        .session(&mut session)
        .await
        .map_err(|_| WarmPoolError::Unavailable)?;
    let anchor = match anchor {
        Some(row) => row,
        None => store
            .targets
            .find_one(release_scope(store, true))
            .sort(doc! {"_id":1})
            .session(&mut session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .ok_or(WarmPoolError::Conflict)?,
    };
    let mut anchor_filter = release_scope(store, true);
    anchor_filter.insert("_id", &anchor.id);
    store
        .check_fenced_write(
            store
                .targets
                .update_one(
                    anchor_filter,
                    doc! {"$inc":{"machine_plan_selection_fence":1_i64}},
                )
                .session(&mut session)
                .await,
        )
        .await?;

    let mut exact = release_scope(store, true);
    exact.extend(doc! {"_id":&candidate.id,"retiring":{"$ne":true},"retired":{"$ne":true}});
    let row = store
        .targets
        .find_one(exact.clone())
        .session(&mut session)
        .await
        .map_err(|_| WarmPoolError::Unavailable)?
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
            .session(&mut session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .and_then(|row| row.machine_plan_activated_at);
        if row.activated_at.is_none() || previously_selected.is_none() {
            let previous = store
                .targets
                .find_one(image_scope)
                .sort(doc! {"activated_at":1})
                .session(&mut session)
                .await
                .map_err(|_| WarmPoolError::Unavailable)?
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
            store
                .check_fenced_write(
                    store
                        .targets
                        .update_one(exact, vec![doc! {"$set":fields}])
                        .session(&mut session)
                        .await,
                )
                .await?;
        }
    }
    session
        .commit_guarded()
        .await
        .map_err(|_| WarmPoolError::Unavailable)
}
