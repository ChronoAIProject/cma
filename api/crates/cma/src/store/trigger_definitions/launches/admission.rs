use layer2_core::trigger_domain::{
    Launch, LaunchAdmission, LaunchAdmissionResult, LaunchEvent, LaunchState, LaunchStoreError,
};
use mongodb::ClientSession;

use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
};

use super::counters::{self, CounterError, CounterUpdate};
use super::documents::{event_document, launch_document, StoredLaunch};
use super::event_log;
use super::queries::{self, ReadError};
use super::transactions;
use super::validation;
use super::{unavailable, MongoTriggerDefinitionStore, TRANSACTION_ATTEMPTS};

#[derive(Clone)]
struct AdmissionReceipt {
    launch: StoredLaunch,
    events: Vec<LaunchEvent>,
    trigger: CounterUpdate,
}

pub(super) async fn admit(
    store: &MongoTriggerDefinitionStore,
    admission: LaunchAdmission,
) -> Result<LaunchAdmissionResult, LaunchStoreError> {
    if let Some(existing) = queries::by_command(
        store,
        &admission.launch.owner,
        &admission.launch.public_id,
        &admission.launch.command_key_hash,
    )
    .await?
    {
        return replay(existing, &admission.launch);
    }
    validation::admission(&admission)?;
    let mut launch = admission.launch;
    let event_at = launch.created_at.clone();
    let events = validation::append_events(&mut launch, admission.events, event_at)?;
    launch.validate()?;

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
        let receipt =
            match admit_once(store, &admission.trigger, &launch, &events, &mut session).await {
                Ok(Attempt::Existing(existing)) => {
                    let _ = session.abort_transaction().await;
                    return replay(*existing, &launch);
                }
                Ok(Attempt::Created(receipt)) => *receipt,
                Err(AttemptError::Retry(reason)) => {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(reason.into());
                    continue;
                }
                Err(AttemptError::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(AttemptError::Driver(error))
                    if is_transient(&error) || is_duplicate_key(&error) =>
                {
                    let duplicate = is_duplicate_key(&error);
                    let _ = session.abort_transaction().await;
                    if duplicate {
                        if let Some(result) = classify_duplicate(store, &launch).await? {
                            return result;
                        }
                    }
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(AttemptError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(unavailable(error));
                }
            };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => {
                return Ok(LaunchAdmissionResult::Created(receipt.launch.launch));
            }
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                let at = session.operation_time().ok_or_else(|| {
                    transactions::no_operation_time("Launch admission commit", error.clone())
                })?;
                let store = store.clone();
                return transactions::reconcile("Launch admission commit", error, async move {
                    reconcile(&store, &receipt, at).await
                })
                .await;
            }
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
        }
    }
    Err(LaunchStoreError::Unavailable(format!(
        "Launch admission exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

async fn admit_once(
    store: &MongoTriggerDefinitionStore,
    trigger: &layer2_core::trigger_domain::PublicTrigger,
    launch: &Launch,
    events: &[LaunchEvent],
    session: &mut ClientSession,
) -> Result<Attempt, AttemptError> {
    if let Some(existing) = queries::command_in_session(
        store,
        &launch.owner,
        &launch.public_id,
        &launch.command_key_hash,
        session,
    )
    .await
    .map_err(AttemptError::from)?
    {
        return Ok(Attempt::Existing(Box::new(existing)));
    }
    if queries::launch_in_session(store, &launch.id, session)
        .await
        .map_err(AttemptError::from)?
        .is_some()
    {
        return Err(AttemptError::Domain(LaunchStoreError::Duplicate));
    }
    if event_log::exists_in_session(store, &launch.id, session)
        .await
        .map_err(AttemptError::from)?
    {
        return Err(AttemptError::Domain(super::corrupt(
            "new Launch ID already has orphan event rows",
        )));
    }
    super::visitor_quota::admit(&store.launch_data, launch, session).await?;
    let ceilings = platform_open(store, session).await?;
    let home_capacity_admission = crate::store::launch_home_capacity::reserve(
        &store.launch_data.admissions,
        &store.launch_data.sandboxes,
        store.global_sandbox_limit,
        &ceilings,
        launch,
        session,
    )
    .await
    .map_err(AttemptError::from)?;
    let trigger =
        counters::record_admission(store, trigger, launch.state == LaunchState::Ready, session)
            .await
            .map_err(AttemptError::from)?;
    store
        .launch_data
        .launches
        .insert_one(
            launch_document(launch, 1, home_capacity_admission).map_err(AttemptError::Domain)?,
        )
        .session(&mut *session)
        .await
        .map_err(AttemptError::Driver)?;
    super::input_protocol::insert(store, launch, session)
        .await
        .map_err(AttemptError::from)?;
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
            .session(session)
            .await
            .map_err(AttemptError::Driver)?;
    }
    Ok(Attempt::Created(Box::new(AdmissionReceipt {
        launch: StoredLaunch {
            launch: launch.clone(),
            mutation_version: 1,
            home_capacity_admission,
        },
        events: events.to_vec(),
        trigger,
    })))
}

async fn classify_duplicate(
    store: &MongoTriggerDefinitionStore,
    intended: &Launch,
) -> Result<Option<Result<LaunchAdmissionResult, LaunchStoreError>>, LaunchStoreError> {
    if let Some(existing) = queries::by_command(
        store,
        &intended.owner,
        &intended.public_id,
        &intended.command_key_hash,
    )
    .await?
    {
        return Ok(Some(replay(existing, intended)));
    }
    if queries::by_id(store, &intended.id).await?.is_some() {
        return Ok(Some(Err(LaunchStoreError::Duplicate)));
    }
    Ok(None)
}

async fn reconcile(
    store: &MongoTriggerDefinitionStore,
    receipt: &AdmissionReceipt,
    at: mongodb::bson::Timestamp,
) -> Result<LaunchAdmissionResult, LaunchStoreError> {
    let mut session = transactions::snapshot_session(store, at).await?;
    let launch = queries::snapshot_launch(store, &receipt.launch.launch.id, &mut session).await?;
    let events_match =
        event_log::snapshot_matches(store, &receipt.launch.launch, &receipt.events, &mut session)
            .await?;
    let trigger_matches = counters::snapshot_matches(store, &receipt.trigger, &mut session).await?;
    if launch.as_ref() == Some(&receipt.launch) && events_match && trigger_matches {
        return Ok(LaunchAdmissionResult::Created(
            receipt.launch.launch.clone(),
        ));
    }
    Err(transactions::still_unknown(
        "Launch admission commit",
        "the commit snapshot did not contain the exact admission",
    ))
}

fn replay(existing: Launch, intended: &Launch) -> Result<LaunchAdmissionResult, LaunchStoreError> {
    if existing.command_fingerprint != intended.command_fingerprint {
        Err(LaunchStoreError::CommandConflict)
    } else {
        Ok(LaunchAdmissionResult::Existing(existing))
    }
}

/// A Launch carries no model liability: read the platform admission state in
/// this transaction without the settings fence (contract §4.1).
async fn platform_open(
    store: &MongoTriggerDefinitionStore,
    session: &mut ClientSession,
) -> Result<Result<layer2_core::admission::AdmissionCeilings, ()>, AttemptError> {
    use crate::store::agent_conversation_store::ApplyError;
    let data = &store.launch_data;
    let now = crate::store::budget::now_seconds();
    crate::store::budget::require_open_with_collections_and_ceilings(
        &data.settings,
        &data.model_budget,
        now,
        layer2_core::admission::AdmissionCause::Fresh,
        session,
    )
    .await
    .map_err(|error| match error {
        ApplyError::Domain(layer2_core::sandbox::store::SandboxStoreError::Budget(reason)) => {
            AttemptError::Domain(LaunchStoreError::Budget(reason))
        }
        ApplyError::Domain(layer2_core::sandbox::store::SandboxStoreError::AdmissionPaused) => {
            AttemptError::Domain(LaunchStoreError::AdmissionPaused)
        }
        ApplyError::Domain(error) => {
            AttemptError::Domain(LaunchStoreError::Unavailable(error.to_string()))
        }
        ApplyError::Driver(error) => AttemptError::Driver(error),
    })
}

enum Attempt {
    Existing(Box<Launch>),
    Created(Box<AdmissionReceipt>),
}

pub(super) enum AttemptError {
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
