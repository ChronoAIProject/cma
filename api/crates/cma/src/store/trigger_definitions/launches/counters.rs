use layer2_core::trigger_domain::{
    PublicTrigger, Trigger, TriggerAvailability, TriggerId, TriggerStoreError,
};
use mongodb::bson::doc;
use mongodb::ClientSession;

use crate::store::sandbox_mongo::signed;
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

use super::super::documents::{read_trigger, trigger_document};
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
    replace(store, changed, stored.mutation_version, session).await
}

pub(super) async fn record_ready(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    session: &mut ClientSession,
) -> Result<CounterUpdate, CounterError> {
    let document = store
        .definitions
        .find_one(doc! { "_id": id.as_str() })
        .session(&mut *session)
        .await
        .map_err(CounterError::Driver)?
        .ok_or(CounterError::Domain(
            layer2_core::trigger_domain::LaunchStoreError::TriggerUnavailable,
        ))?;
    let stored = read_trigger(&document)
        .map_err(trigger_error)
        .map_err(CounterError::Domain)?;
    verify_revisions(store, &stored.trigger, session).await?;
    let mut changed = stored.trigger;
    changed
        .record_agent_ready()
        .map_err(layer2_core::trigger_domain::LaunchStoreError::from)
        .map_err(CounterError::Domain)?;
    replace(store, changed, stored.mutation_version, session).await
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
    session: &mut ClientSession,
) -> Result<CounterUpdate, CounterError> {
    changed
        .validate()
        .map_err(layer2_core::trigger_domain::LaunchStoreError::from)
        .map_err(CounterError::Domain)?;
    let next = mutation_version
        .checked_add(1)
        .ok_or_else(|| CounterError::Domain(corrupt("Trigger mutation version is exhausted")))?;
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
            trigger_document(&changed, next)
                .map_err(trigger_error)
                .map_err(CounterError::Domain)?,
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
