//! Keep usable draft pins and their definition CAS in one transaction.
use layer2_core::{
    trigger_domain::{
        Trigger, TriggerContent, TriggerId, TriggerReferenceError, TriggerStoreError,
    },
    SubjectId,
};
use mongodb::bson::doc;

use super::{documents, driver_error, queries, MongoTriggerDefinitionStore, MUTATION_ATTEMPTS};
use crate::store::{
    profile_draft_admission::{self, Error, Mutation, Pins},
    sandbox_mongo::{is_duplicate_key, signed},
    schema::DOCUMENT_SCHEMA_VERSION,
};

pub(super) async fn create(
    store: &MongoTriggerDefinitionStore,
    trigger: &Trigger,
) -> Result<(), TriggerStoreError> {
    let db = store.client.database(&store.definitions.namespace().db);
    let mut pins = Pins::trigger(&trigger.draft.target);
    profile_draft_admission::transact(
        &store.client,
        &db,
        &mut pins,
        &Mutation::Insert {
            collection: store.definitions.clone(),
            document: documents::trigger_document(trigger, 1)?,
        },
    )
    .await
    .map_err(|error| map_error("create Trigger definition", error))?;
    Ok(())
}

pub(super) async fn save(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    owner: &SubjectId,
    expected: u64,
    draft: TriggerContent,
    updated_at: String,
) -> Result<Trigger, TriggerStoreError> {
    let db = store.client.database(&store.definitions.namespace().db);
    let mut pins = Pins::trigger(&draft.target);
    pins.guard(
        super::TRIGGER_DEFINITIONS,
        doc! {
            "_id":id.as_str(), "owner":owner.as_str(),
            "account_erasure_fenced":{"$ne":true},
        },
    );
    for attempt in 0..MUTATION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut stored = queries::mutable_owned(store, id, owner)
            .await?
            .ok_or(TriggerStoreError::NotFound)?;
        if stored.trigger.draft_version != expected {
            return Err(TriggerStoreError::Conflict {
                expected,
                actual: stored.trigger.draft_version,
            });
        }
        stored
            .trigger
            .replace_draft(expected, draft.clone(), updated_at.clone())?;
        stored.trigger.validate()?;
        let next = stored.mutation_version.checked_add(1).ok_or_else(|| {
            TriggerStoreError::Corrupt("Trigger mutation version is exhausted".into())
        })?;
        let mutation = Mutation::Replace {
            collection: store.definitions.clone(),
            filter: doc! {
                "_id":id.as_str(),"owner":owner.as_str(),
                "document_schema_version":DOCUMENT_SCHEMA_VERSION,
                "account_erasure_fenced":{"$ne":true},
                "mutation_version":signed(stored.mutation_version,"Trigger mutation version").map_err(TriggerStoreError::Corrupt)?,
            },
            document: documents::trigger_document(&stored.trigger, next)?,
        };
        if profile_draft_admission::transact(&store.client, &db, &mut pins, &mutation)
            .await
            .map_err(|error| map_error("mutate Trigger definition", error))?
            .is_some()
        {
            return Ok(stored.trigger);
        }
    }
    Err(TriggerStoreError::Unavailable(
        "Trigger definition kept changing during mutation".into(),
    ))
}

fn map_error(context: &str, error: Error) -> TriggerStoreError {
    match error {
        Error::ReferenceUnavailable => TriggerReferenceError::Unavailable {
            field: "draft.target",
        }
        .into(),
        Error::Driver(error) if is_duplicate_key(&error) => TriggerStoreError::Duplicate,
        Error::Driver(error) => driver_error(context, error),
    }
}
