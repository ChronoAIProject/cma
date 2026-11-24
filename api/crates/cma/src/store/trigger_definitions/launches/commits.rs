use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::trigger_domain::{Launch, LaunchCommit, LaunchEvent, LaunchStoreError};
use mongodb::bson::doc;
use mongodb::ClientSession;

use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction, CommitOutcome,
};
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

use super::counters::{self, CounterError, CounterUpdate};
use super::documents::{event_document, launch_document, StoredLaunch};
use super::event_log;
use super::queries::{self, ReadError};
use super::transactions;
use super::validation;
use super::{corrupt, unavailable, MongoTriggerDefinitionStore, TRANSACTION_ATTEMPTS};

#[derive(Clone)]
struct CommitReceipt {
    launch: StoredLaunch,
    events: Vec<LaunchEvent>,
    trigger: Option<CounterUpdate>,
}

pub(super) async fn commit(
    store: &MongoTriggerDefinitionStore,
    commit: LaunchCommit,
) -> Result<Launch, LaunchStoreError> {
    let mut last_transient = None;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(unavailable(error)),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_transient = Some(error.to_string());
                continue;
            }
            return Err(unavailable(error));
        }
        let receipt = match commit_once(store, &commit, &mut session).await {
            Ok(receipt) => receipt,
            Err(AttemptError::Retry(reason)) => {
                let _ = session.abort_guarded().await;
                last_transient = Some(reason.into());
                continue;
            }
            Err(AttemptError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(AttemptError::Driver(error))
                if is_transient(&error) || is_duplicate_key(&error) =>
            {
                let _ = session.abort_guarded().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(AttemptError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                return Err(unavailable(error));
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(receipt.launch.launch),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                let at = session.operation_time().ok_or_else(|| {
                    transactions::no_operation_time("Launch commit", error.clone())
                })?;
                let store = store.clone();
                return transactions::reconcile("Launch commit", error, async move {
                    reconcile(&store, &receipt, at).await
                })
                .await;
            }
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
        }
    }
    Err(LaunchStoreError::Unavailable(format!(
        "Launch commit exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

async fn commit_once(
    store: &MongoTriggerDefinitionStore,
    commit: &LaunchCommit,
    session: &mut ClientSession,
) -> Result<CommitReceipt, AttemptError> {
    let current = queries::launch_in_session(store, &commit.launch.id, session)
        .await
        .map_err(AttemptError::from)?
        .ok_or(AttemptError::Domain(LaunchStoreError::NotFound))?;
    // Like lease claims, an exact ready-monitor deferral changes only work
    // metadata. It neither appends history nor authorizes a resource effect.
    // Every other transition retains complete event-log validation.
    if !validation::is_ready_observation(&current.launch, commit) {
        event_log::verify_in_session(store, &current.launch, session)
            .await
            .map_err(AttemptError::from)?;
    }
    if current.launch.revision != commit.expected_revision {
        return Err(AttemptError::Domain(LaunchStoreError::Conflict {
            expected: commit.expected_revision,
            actual: current.launch.revision,
        }));
    }
    validation::immutable_launch(&current.launch, &commit.launch).map_err(AttemptError::Domain)?;
    let expected_revision = current
        .launch
        .revision
        .checked_add(1)
        .ok_or_else(|| AttemptError::Domain(corrupt("Launch revision overflow")))?;
    if commit.launch.revision != expected_revision {
        return Err(AttemptError::Domain(corrupt(
            "Launch commit did not advance exactly one revision",
        )));
    }
    let mut changed = commit.launch.clone();
    let events =
        validation::append_events(&mut changed, commit.events.clone(), commit.event_at.clone())
            .map_err(AttemptError::Domain)?;
    changed
        .validate()
        .map_err(LaunchStoreError::from)
        .map_err(AttemptError::Domain)?;
    if current.home_capacity_admission {
        crate::store::launch_home_capacity::release(
            &store.launch_data.admissions,
            &changed,
            session,
        )
        .await
        .map_err(AttemptError::from)?;
    }
    let trigger = if current.launch.ready_at.is_none() && changed.ready_at.is_some() {
        counters::record_ready(store, &changed, session)
            .await
            .map_err(AttemptError::from)?
    } else {
        None
    };
    if !events.is_empty() {
        let documents = events
            .iter()
            .map(event_document)
            .collect::<Result<Vec<_>, _>>()
            .map_err(AttemptError::Domain)?;
        store
            .launch_data
            .events
            .insert_many(documents)
            .session(&mut *session)
            .await
            .map_err(AttemptError::Driver)?;
    }
    let next_mutation = current
        .mutation_version
        .checked_add(1)
        .ok_or_else(|| AttemptError::Domain(corrupt("Launch mutation version is exhausted")))?;
    let result = store
        .launch_data
        .launches
        .replace_one(
            doc! {
                "_id": current.launch.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": signed(current.launch.revision, "Launch revision")
                    .map_err(corrupt)
                    .map_err(AttemptError::Domain)?,
                "event_head": signed(current.launch.event_head, "Launch event head")
                    .map_err(corrupt)
                    .map_err(AttemptError::Domain)?,
                "mutation_version": signed(current.mutation_version, "Launch mutation version")
                    .map_err(corrupt)
                    .map_err(AttemptError::Domain)?,
            },
            launch_document(&changed, next_mutation, current.home_capacity_admission)
                .map_err(AttemptError::Domain)?,
        )
        .session(session)
        .await
        .map_err(AttemptError::Driver)?;
    if result.matched_count != 1 {
        return Err(AttemptError::Retry(
            "Launch changed during revision-fenced commit",
        ));
    }
    Ok(CommitReceipt {
        launch: StoredLaunch {
            launch: changed,
            mutation_version: next_mutation,
            home_capacity_admission: current.home_capacity_admission,
        },
        events,
        trigger,
    })
}

async fn reconcile(
    store: &MongoTriggerDefinitionStore,
    receipt: &CommitReceipt,
    at: mongodb::bson::Timestamp,
) -> Result<Launch, LaunchStoreError> {
    let mut session = transactions::snapshot_session(store, at).await?;
    let launch = queries::snapshot_launch(store, &receipt.launch.launch.id, &mut session).await?;
    let events_match =
        event_log::snapshot_matches(store, &receipt.launch.launch, &receipt.events, &mut session)
            .await?;
    let trigger_matches = match &receipt.trigger {
        Some(trigger) => counters::snapshot_matches(store, trigger, &mut session).await?,
        None => true,
    };
    if launch.as_ref() == Some(&receipt.launch) && events_match && trigger_matches {
        return Ok(receipt.launch.launch.clone());
    }
    Err(transactions::still_unknown(
        "Launch commit",
        "the commit snapshot did not contain the exact aggregate and events",
    ))
}

enum AttemptError {
    Retry(&'static str),
    Domain(LaunchStoreError),
    Driver(mongodb::error::Error),
}

impl From<ReadError> for AttemptError {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Domain(error) => Self::Domain(error),
            ReadError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<CounterError> for AttemptError {
    fn from(error: CounterError) -> Self {
        match error {
            CounterError::Retry(reason) => Self::Retry(reason),
            CounterError::Domain(error) => Self::Domain(error),
            CounterError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<crate::store::launch_home_capacity::Error> for AttemptError {
    fn from(error: crate::store::launch_home_capacity::Error) -> Self {
        match error {
            crate::store::launch_home_capacity::Error::Domain(error) => Self::Domain(error),
            crate::store::launch_home_capacity::Error::Driver(error) => Self::Driver(error),
        }
    }
}
