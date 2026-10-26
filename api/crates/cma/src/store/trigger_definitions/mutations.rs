use layer2_core::ids::SubjectId;
use layer2_core::trigger_domain::{
    Trigger, TriggerError, TriggerId, TriggerStats, TriggerStoreError,
};
use mongodb::bson::{doc, Bson};
use mongodb::options::ReturnDocument;

use super::super::sandbox_mongo::{is_duplicate_key, signed};
use super::super::schema::DOCUMENT_SCHEMA_VERSION;
use super::documents::{read_trigger, trigger_document};
use super::queries;
use super::{driver_error, MongoTriggerDefinitionStore, MUTATION_ATTEMPTS};

pub(super) async fn delete(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    owner: &SubjectId,
    archived_at: String,
) -> Result<(), TriggerStoreError> {
    let result = store
        .definitions
        .delete_one(doc! {
            "_id": id.as_str(),
            "owner": owner.as_str(),
            "availability": "draft",
            "public_id": Bson::Null,
            "latest_revision": 0_i64,
        })
        .await
        .map_err(|error| driver_error("delete unpublished Trigger definition", error))?;
    if result.deleted_count == 1 {
        return Ok(());
    }
    owned(store, id, owner, |trigger| {
        trigger.archive(archived_at.clone())?;
        Ok(())
    })
    .await
}

pub(super) async fn owned<T>(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    owner: &SubjectId,
    change: impl Fn(&mut Trigger) -> Result<T, TriggerStoreError>,
) -> Result<T, TriggerStoreError> {
    for attempt in 0..MUTATION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut stored = queries::owned(store, id, owner)
            .await?
            .ok_or(TriggerStoreError::NotFound)?;
        let result = change(&mut stored.trigger)?;
        stored.trigger.validate()?;
        let next = stored.mutation_version.checked_add(1).ok_or_else(|| {
            TriggerStoreError::Corrupt("Trigger mutation version is exhausted".into())
        })?;
        let expected = signed(stored.mutation_version, "Trigger mutation version")
            .map_err(TriggerStoreError::Corrupt)?;
        let replacement = trigger_document(&stored.trigger, next)?;
        match store
            .definitions
            .replace_one(
                doc! {
                    "_id": id.as_str(),
                    "owner": owner.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "mutation_version": expected,
                },
                replacement,
            )
            .await
        {
            Ok(outcome) if outcome.matched_count == 1 => return Ok(result),
            Ok(_) => continue,
            Err(error) if is_duplicate_key(&error) => return Err(TriggerStoreError::Duplicate),
            Err(error) => return Err(driver_error("mutate Trigger definition", error)),
        }
    }
    Err(TriggerStoreError::Unavailable(
        "Trigger definition kept changing during mutation".into(),
    ))
}

pub(super) async fn record_launch(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
) -> Result<TriggerStats, TriggerStoreError> {
    increment(store, id, Counter::Launch).await
}

pub(super) async fn record_ready(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
) -> Result<TriggerStats, TriggerStoreError> {
    increment(store, id, Counter::Ready).await
}

#[derive(Clone, Copy)]
enum Counter {
    Launch,
    Ready,
}

async fn increment(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    counter: Counter,
) -> Result<TriggerStats, TriggerStoreError> {
    let mut filter = doc! {
        "_id": id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "mutation_version": { "$type": "long", "$gt": 0_i64, "$lt": i64::MAX },
        "launches_total": { "$type": "long", "$gte": 0_i64, "$lt": i64::MAX },
        "agents_ready_total": { "$type": "long", "$gte": 0_i64, "$lt": i64::MAX },
        "record.stats.launches_total": { "$type": "long", "$gte": 0_i64, "$lt": i64::MAX },
        "record.stats.agents_ready_total": { "$type": "long", "$gte": 0_i64, "$lt": i64::MAX },
    };
    let matching_counters = [
        doc! { "$eq": ["$launches_total", "$record.stats.launches_total"] },
        doc! { "$eq": ["$agents_ready_total", "$record.stats.agents_ready_total"] },
    ];
    let update = match counter {
        Counter::Launch => {
            filter.insert("availability", "published");
            filter.insert("record.availability", "published");
            filter.insert(
                "$expr",
                doc! { "$and": [
                    matching_counters[0].clone(),
                    matching_counters[1].clone(),
                    { "$lte": ["$agents_ready_total", "$launches_total"] },
                ] },
            );
            doc! { "$inc": {
                "mutation_version": 1_i64,
                "launches_total": 1_i64,
                "record.stats.launches_total": 1_i64,
            } }
        }
        Counter::Ready => {
            filter.insert(
                "$expr",
                doc! { "$and": [
                    matching_counters[0].clone(),
                    matching_counters[1].clone(),
                    { "$lt": ["$agents_ready_total", "$launches_total"] },
                ] },
            );
            doc! { "$inc": {
                "mutation_version": 1_i64,
                "agents_ready_total": 1_i64,
                "record.stats.agents_ready_total": 1_i64,
            } }
        }
    };
    let document = store
        .definitions
        .find_one_and_update(filter, update)
        .return_document(ReturnDocument::After)
        .await
        .map_err(|error| driver_error("increment Trigger counter", error))?;
    if let Some(document) = document {
        let updated = read_trigger(&document)?;
        queries::verify_revisions(store, std::slice::from_ref(&updated.trigger)).await?;
        return Ok(updated.trigger.stats);
    }
    classify_counter_miss(store, id, counter).await
}

async fn classify_counter_miss(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    counter: Counter,
) -> Result<TriggerStats, TriggerStoreError> {
    let stored = queries::by_id(store, id)
        .await?
        .ok_or(TriggerStoreError::NotFound)?;
    if stored.mutation_version >= i64::MAX as u64 {
        return Err(TriggerStoreError::Corrupt(
            "Trigger mutation version is exhausted".into(),
        ));
    }
    let mut trigger = stored.trigger;
    match counter {
        Counter::Launch => trigger.record_launch()?,
        Counter::Ready => trigger.record_agent_ready()?,
    }
    if trigger.stats.launches_total > i64::MAX as u64 {
        return Err(TriggerError::new("trigger_counter_overflow", "stats.launches_total").into());
    }
    if trigger.stats.agents_ready_total > i64::MAX as u64 {
        return Err(
            TriggerError::new("trigger_counter_overflow", "stats.agents_ready_total").into(),
        );
    }
    Err(TriggerStoreError::Unavailable(
        "Trigger counter update did not match a valid aggregate".into(),
    ))
}
