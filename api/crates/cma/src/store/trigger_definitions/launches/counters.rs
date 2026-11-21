use layer2_core::trigger_domain::{
    Launch, PublicTrigger, Trigger, TriggerAvailability, TriggerReferenceError, TriggerStoreError,
};
use mongodb::bson::{doc, Document};
use mongodb::ClientSession;

use crate::store::sandbox_mongo::signed;
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

use super::super::documents::{read_trigger, trigger_document};
use super::super::publisher_erasure;
use super::super::queries::{self as trigger_queries, TransactionReadError};
use super::{corrupt, trigger_error, MongoTriggerDefinitionStore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CounterUpdate {
    pub trigger: Trigger,
    pub mutation_version: u64,
}

pub(super) async fn record_admission(
    store: &MongoTriggerDefinitionStore,
    expected: &PublicTrigger,
    ready: bool,
    session: &mut ClientSession,
) -> Result<CounterUpdate, CounterError> {
    let document = store
        .definitions
        .find_one(doc! { "public_id": expected.public_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(CounterError::Driver)?
        .ok_or(CounterError::Domain(
            layer2_core::trigger_domain::LaunchStoreError::TriggerUnavailable,
        ))?;
    if document.get_bool("account_erasure_fenced").ok() == Some(true) {
        return Err(CounterError::Domain(
            layer2_core::trigger_domain::LaunchStoreError::TriggerRetired,
        ));
    }
    let stored = read_trigger(&document)
        .map_err(trigger_error)
        .map_err(CounterError::Domain)?;
    verify_revisions(store, &stored.trigger, session).await?;
    if stored.trigger.availability != TriggerAvailability::Published {
        return Err(CounterError::Domain(
            layer2_core::trigger_domain::LaunchStoreError::TriggerRetired,
        ));
    }
    if project(&stored.trigger).map_err(CounterError::Domain)? != *expected {
        return Err(CounterError::Domain(
            layer2_core::trigger_domain::LaunchStoreError::TriggerUnavailable,
        ));
    }

    let mut changed = stored.trigger;
    changed
        .record_launch()
        .map_err(layer2_core::trigger_domain::LaunchStoreError::from)
        .map_err(CounterError::Domain)?;
    if ready {
        changed
            .record_agent_ready()
            .map_err(layer2_core::trigger_domain::LaunchStoreError::from)
            .map_err(CounterError::Domain)?;
    }
    replace(store, changed, stored.mutation_version, &document, session).await
}

pub(super) async fn record_ready(
    store: &MongoTriggerDefinitionStore,
    launch: &Launch,
    session: &mut ClientSession,
) -> Result<Option<CounterUpdate>, CounterError> {
    let Some(document) = store
        .definitions
        .find_one(doc! { "_id": launch.trigger_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(CounterError::Driver)?
    else {
        if !publisher_erasure::prove(
            store,
            &launch.trigger_id,
            &launch.revision_digest,
            &launch.public_id,
        )
        .await
        .map_err(CounterError::from)?
        {
            return Err(CounterError::Domain(corrupt(
                "Launch publisher definition is missing without erasure evidence",
            )));
        }
        // Validated erasure evidence permits publisher statistics to disappear
        // while the visitor's Agent and Launch retain runtime authority.
        return Ok(None);
    };
    let stored = read_trigger(&document)
        .map_err(trigger_error)
        .map_err(CounterError::Domain)?;
    match trigger_queries::verify_in_session(store, &stored.trigger, session).await {
        Ok(()) => {}
        Err(TransactionReadError::Domain(TriggerStoreError::Reference(
            TriggerReferenceError::Unavailable { .. },
        ))) => return Ok(None),
        Err(TransactionReadError::Domain(error)) => {
            return Err(CounterError::Domain(trigger_error(error)));
        }
        Err(TransactionReadError::Driver(error)) => return Err(CounterError::Driver(error)),
    }
    let mut changed = stored.trigger;
    changed
        .record_agent_ready()
        .map_err(layer2_core::trigger_domain::LaunchStoreError::from)
        .map_err(CounterError::Domain)?;
    replace(store, changed, stored.mutation_version, &document, session)
        .await
        .map(Some)
}

pub(super) async fn snapshot_matches(
    store: &MongoTriggerDefinitionStore,
    expected: &CounterUpdate,
    session: &mut ClientSession,
) -> Result<bool, layer2_core::trigger_domain::LaunchStoreError> {
    use mongodb::options::ReadConcern;

    let document = store
        .definitions
        .find_one(doc! { "_id": expected.trigger.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(session)
        .await
        .map_err(super::unavailable)?;
    let Some(document) = document else {
        return Ok(false);
    };
    let stored = read_trigger(&document).map_err(trigger_error)?;
    Ok(stored.trigger == expected.trigger && stored.mutation_version == expected.mutation_version)
}

async fn replace(
    store: &MongoTriggerDefinitionStore,
    changed: Trigger,
    mutation_version: u64,
    original: &Document,
    session: &mut ClientSession,
) -> Result<CounterUpdate, CounterError> {
    changed
        .validate()
        .map_err(layer2_core::trigger_domain::LaunchStoreError::from)
        .map_err(CounterError::Domain)?;
    let next = mutation_version
        .checked_add(1)
        .ok_or_else(|| CounterError::Domain(corrupt("Trigger mutation version is exhausted")))?;
    let mut replacement = trigger_document(&changed, next)
        .map_err(trigger_error)
        .map_err(CounterError::Domain)?;
    for field in ["account_erasure_fenced", "account_erasure_fenced_at"] {
        if let Some(value) = original.get(field) {
            replacement.insert(field, value.clone());
        }
    }
    let result = store
        .definitions
        .replace_one(
            doc! {
                "_id": changed.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "mutation_version": signed(mutation_version, "Trigger mutation version")
                    .map_err(corrupt)
                    .map_err(CounterError::Domain)?,
            },
            replacement,
        )
        .session(session)
        .await
        .map_err(CounterError::Driver)?;
    if result.matched_count != 1 {
        return Err(CounterError::Retry(
            "Trigger changed while recording Launch counters",
        ));
    }
    Ok(CounterUpdate {
        trigger: changed,
        mutation_version: next,
    })
}

async fn verify_revisions(
    store: &MongoTriggerDefinitionStore,
    trigger: &Trigger,
    session: &mut ClientSession,
) -> Result<(), CounterError> {
    match trigger_queries::verify_in_session(store, trigger, session).await {
        Ok(()) => Ok(()),
        Err(TransactionReadError::Domain(error)) => Err(CounterError::Domain(trigger_error(error))),
        Err(TransactionReadError::Driver(error)) => Err(CounterError::Driver(error)),
    }
}

fn project(
    trigger: &Trigger,
) -> Result<PublicTrigger, layer2_core::trigger_domain::LaunchStoreError> {
    let public_id = trigger
        .public_id
        .clone()
        .ok_or_else(|| corrupt("published Trigger has no public ID"))?;
    let revision = trigger
        .latest_revision()
        .cloned()
        .ok_or_else(|| corrupt("published Trigger has no immutable revision"))?;
    Ok(PublicTrigger {
        trigger_id: trigger.id.clone(),
        public_id,
        owner: trigger.owner.clone(),
        availability: trigger.availability,
        revision,
    })
}

pub(super) enum CounterError {
    Retry(&'static str),
    Domain(layer2_core::trigger_domain::LaunchStoreError),
    Driver(mongodb::error::Error),
}

impl From<TriggerStoreError> for CounterError {
    fn from(error: TriggerStoreError) -> Self {
        Self::Domain(trigger_error(error))
    }
}
