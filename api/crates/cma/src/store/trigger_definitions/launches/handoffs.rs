use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::ids::{ServiceHandoffId, SubjectId};
use layer2_core::trigger_domain::{LaunchStoreError, ServiceConnectionHandoff};
use mongodb::bson::doc;
use mongodb::ClientSession;

use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction, CommitOutcome,
};
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

use super::documents::{handoff_document, launch_document, read_handoff, StoredLaunch};
use super::queries::{self, ReadError};
use super::transactions;
use super::validation;
use super::{corrupt, unavailable, MongoTriggerDefinitionStore, TRANSACTION_ATTEMPTS};

#[derive(Clone)]
struct CreationReceipt {
    handoff: ServiceConnectionHandoff,
    launch: StoredLaunch,
}

pub(super) async fn create(
    store: &MongoTriggerDefinitionStore,
    handoff: ServiceConnectionHandoff,
) -> Result<ServiceConnectionHandoff, LaunchStoreError> {
    handoff.validate()?;
    let mut last_transient = None;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut session = begin(store, &mut last_transient).await?;
        let Some(mut session) = session.take() else {
            continue;
        };
        let receipt = match create_once(store, &handoff, &mut session).await {
            Ok(CreateAttempt::Existing(existing)) => {
                let _ = session.abort_guarded().await;
                return replay(*existing, &handoff);
            }
            Ok(CreateAttempt::Created(receipt)) => *receipt,
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
            CommitOutcome::Committed => return Ok(receipt.handoff),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                let at = session.operation_time().ok_or_else(|| {
                    transactions::no_operation_time("service handoff creation", error.clone())
                })?;
                let store = store.clone();
                return transactions::reconcile("service handoff creation", error, async move {
                    reconcile_creation(&store, &receipt, at).await
                })
                .await;
            }
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
        }
    }
    Err(exhausted("creation", last_transient))
}

pub(super) async fn owned(
    store: &MongoTriggerDefinitionStore,
    id: &ServiceHandoffId,
    owner: &SubjectId,
) -> Result<Option<ServiceConnectionHandoff>, LaunchStoreError> {
    let document = store
        .launch_data
        .handoffs
        .find_one(doc! { "_id": id.as_str(), "owner": owner.as_str() })
        .await
        .map_err(unavailable)?;
    let Some(document) = document else {
        return Ok(None);
    };
    let handoff = read_handoff(&document)?;
    if &handoff.id != id || &handoff.owner != owner {
        return Err(corrupt("owned service handoff query crossed its boundary"));
    }
    Ok(Some(handoff))
}

pub(super) async fn save(
    store: &MongoTriggerDefinitionStore,
    expected_revision: u64,
    handoff: ServiceConnectionHandoff,
) -> Result<ServiceConnectionHandoff, LaunchStoreError> {
    handoff.validate()?;
    let mut last_transient = None;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut session = begin(store, &mut last_transient).await?;
        let Some(mut session) = session.take() else {
            continue;
        };
        let saved = match save_once(store, expected_revision, &handoff, &mut session).await {
            Ok(saved) => saved,
            Err(AttemptError::Retry(reason)) => {
                let _ = session.abort_guarded().await;
                last_transient = Some(reason.into());
                continue;
            }
            Err(AttemptError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(AttemptError::Driver(error)) if is_transient(&error) => {
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
            CommitOutcome::Committed => return Ok(saved),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                let at = session.operation_time().ok_or_else(|| {
                    transactions::no_operation_time("service handoff save", error.clone())
                })?;
                let store = store.clone();
                return transactions::reconcile("service handoff save", error, async move {
                    reconcile_save(&store, &saved, at).await
                })
                .await;
            }
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
        }
    }
    Err(exhausted("save", last_transient))
}

async fn create_once(
    store: &MongoTriggerDefinitionStore,
    handoff: &ServiceConnectionHandoff,
    session: &mut ClientSession,
) -> Result<CreateAttempt, AttemptError> {
    let launch = queries::launch_in_session(store, &handoff.launch_id, session)
        .await
        .map_err(AttemptError::from)?
        .filter(|row| row.launch.owner == handoff.owner)
        .ok_or(AttemptError::Domain(LaunchStoreError::NotFound))?;
    if launch.launch.state.is_terminal() {
        return Err(AttemptError::Domain(LaunchStoreError::NotFound));
    }
    if let Some(existing) = queries::handoff_command_in_session(
        store,
        &handoff.owner,
        &handoff.launch_id,
        &handoff.intent_id,
        session,
    )
    .await
    .map_err(AttemptError::from)?
    {
        return Ok(CreateAttempt::Existing(Box::new(existing)));
    }
    if queries::handoff_in_session(store, &handoff.id, session)
        .await
        .map_err(AttemptError::from)?
        .is_some()
    {
        return Err(AttemptError::Domain(LaunchStoreError::Duplicate));
    }
    let next_mutation = launch
        .mutation_version
        .checked_add(1)
        .ok_or_else(|| AttemptError::Domain(corrupt("Launch mutation version is exhausted")))?;
    let result = store
        .launch_data
        .launches
        .replace_one(
            doc! {
                "_id": launch.launch.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "mutation_version": signed(launch.mutation_version, "Launch mutation version")
                    .map_err(corrupt)
                    .map_err(AttemptError::Domain)?,
            },
            launch_document(
                &launch.launch,
                next_mutation,
                launch.home_capacity_admission,
            )
            .map_err(AttemptError::Domain)?,
        )
        .session(&mut *session)
        .await
        .map_err(AttemptError::Driver)?;
    if result.matched_count != 1 {
        return Err(AttemptError::Retry(
            "Launch changed during service handoff creation",
        ));
    }
    store
        .launch_data
        .handoffs
        .insert_one(handoff_document(handoff).map_err(AttemptError::Domain)?)
        .session(session)
        .await
        .map_err(AttemptError::Driver)?;
    Ok(CreateAttempt::Created(Box::new(CreationReceipt {
        handoff: handoff.clone(),
        launch: StoredLaunch {
            launch: launch.launch,
            mutation_version: next_mutation,
            home_capacity_admission: launch.home_capacity_admission,
        },
    })))
}

async fn save_once(
    store: &MongoTriggerDefinitionStore,
    expected_revision: u64,
    handoff: &ServiceConnectionHandoff,
    session: &mut ClientSession,
) -> Result<ServiceConnectionHandoff, AttemptError> {
    let current = queries::handoff_in_session(store, &handoff.id, session)
        .await
        .map_err(AttemptError::from)?
        .ok_or(AttemptError::Domain(LaunchStoreError::NotFound))?;
    if current.revision != expected_revision {
        return Err(AttemptError::Domain(LaunchStoreError::Conflict {
            expected: expected_revision,
            actual: current.revision,
        }));
    }
    validation::immutable_handoff(&current, handoff).map_err(AttemptError::Domain)?;
    let next = current
        .revision
        .checked_add(1)
        .ok_or_else(|| AttemptError::Domain(corrupt("service handoff revision overflow")))?;
    if handoff.revision != next {
        return Err(AttemptError::Domain(corrupt(
            "service handoff did not advance exactly one revision",
        )));
    }
    let result = store
        .launch_data
        .handoffs
        .replace_one(
            doc! {
                "_id": handoff.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": signed(current.revision, "service handoff revision")
                    .map_err(corrupt)
                    .map_err(AttemptError::Domain)?,
            },
            handoff_document(handoff).map_err(AttemptError::Domain)?,
        )
        .session(session)
        .await
        .map_err(AttemptError::Driver)?;
    if result.matched_count != 1 {
        return Err(AttemptError::Retry(
            "service handoff changed during revision-fenced save",
        ));
    }
    Ok(handoff.clone())
}

async fn reconcile_creation(
    store: &MongoTriggerDefinitionStore,
    receipt: &CreationReceipt,
    at: mongodb::bson::Timestamp,
) -> Result<ServiceConnectionHandoff, LaunchStoreError> {
    let mut session = transactions::snapshot_session(store, at).await?;
    let launch = queries::snapshot_launch(store, &receipt.launch.launch.id, &mut session).await?;
    let handoff = queries::snapshot_handoff(store, &receipt.handoff.id, &mut session).await?;
    if launch.as_ref() == Some(&receipt.launch) && handoff.as_ref() == Some(&receipt.handoff) {
        return Ok(receipt.handoff.clone());
    }
    Err(transactions::still_unknown(
        "service handoff creation",
        "the commit snapshot did not contain the exact handoff and Launch fence",
    ))
}

async fn reconcile_save(
    store: &MongoTriggerDefinitionStore,
    expected: &ServiceConnectionHandoff,
    at: mongodb::bson::Timestamp,
) -> Result<ServiceConnectionHandoff, LaunchStoreError> {
    let mut session = transactions::snapshot_session(store, at).await?;
    let handoff = queries::snapshot_handoff(store, &expected.id, &mut session).await?;
    if handoff.as_ref() == Some(expected) {
        return Ok(expected.clone());
    }
    Err(transactions::still_unknown(
        "service handoff save",
        "the commit snapshot did not contain the exact saved handoff",
    ))
}

async fn begin(
    store: &MongoTriggerDefinitionStore,
    last_transient: &mut Option<String>,
) -> Result<Option<ClientSession>, LaunchStoreError> {
    let mut session = match store.client.start_session().await {
        Ok(session) => session,
        Err(error) if is_transient(&error) => {
            *last_transient = Some(error.to_string());
            return Ok(None);
        }
        Err(error) => return Err(unavailable(error)),
    };
    match start_transaction(&mut session).await {
        Ok(()) => Ok(Some(session)),
        Err(error) if is_transient(&error) => {
            *last_transient = Some(error.to_string());
            Ok(None)
        }
        Err(error) => Err(unavailable(error)),
    }
}

fn replay(
    existing: ServiceConnectionHandoff,
    intended: &ServiceConnectionHandoff,
) -> Result<ServiceConnectionHandoff, LaunchStoreError> {
    if existing.service_id != intended.service_id {
        Err(LaunchStoreError::CommandConflict)
    } else {
        Ok(existing)
    }
}

fn exhausted(operation: &str, last: Option<String>) -> LaunchStoreError {
    LaunchStoreError::Unavailable(format!(
        "service handoff {operation} exhausted its retry budget: {}",
        last.unwrap_or_else(|| "no driver result".into())
    ))
}

enum CreateAttempt {
    Existing(Box<ServiceConnectionHandoff>),
    Created(Box<CreationReceipt>),
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
