use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::ids::LaunchId;
use layer2_core::trigger_domain::{Launch, LaunchStoreError};
use mongodb::bson::doc;
use mongodb::ClientSession;

use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, signed, start_transaction, CommitOutcome,
};
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

use super::documents::{launch_document, StoredLaunch};
use super::queries::{self, ReadError};
use super::transactions;
use super::{corrupt, unavailable, MongoTriggerDefinitionStore, TRANSACTION_ATTEMPTS};

pub(super) async fn claim(
    store: &MongoTriggerDefinitionStore,
    id: &LaunchId,
    holder: &str,
    now: String,
    lease_expires_at: String,
) -> Result<Option<Launch>, LaunchStoreError> {
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
            match claim_once(store, id, holder, &now, &lease_expires_at, &mut session).await {
                Ok(None) => {
                    let _ = session.abort_guarded().await;
                    return Ok(None);
                }
                Ok(Some(receipt)) => receipt,
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
            CommitOutcome::Committed => return Ok(Some(receipt.launch)),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                let at = session.operation_time().ok_or_else(|| {
                    transactions::no_operation_time("Launch lease claim", error.clone())
                })?;
                let store = store.clone();
                return transactions::reconcile("Launch lease claim", error, async move {
                    reconcile(&store, &receipt, at).await.map(Some)
                })
                .await;
            }
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
        }
    }
    Err(LaunchStoreError::Unavailable(format!(
        "Launch lease claim exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

async fn claim_once(
    store: &MongoTriggerDefinitionStore,
    id: &LaunchId,
    holder: &str,
    now: &str,
    lease_expires_at: &str,
    session: &mut ClientSession,
) -> Result<Option<StoredLaunch>, AttemptError> {
    let Some(current) = queries::launch_in_session(store, id, session)
        .await
        .map_err(AttemptError::from)?
    else {
        return Ok(None);
    };
    if !current.launch.is_worker_due(now) {
        return Ok(None);
    }
    let mut changed = current.launch.clone();
    changed
        .work
        .claim(holder.to_owned(), now, lease_expires_at.to_owned())
        .map_err(|_| AttemptError::Domain(LaunchStoreError::LeaseHeld))?;
    changed.revision = changed
        .revision
        .checked_add(1)
        .ok_or_else(|| AttemptError::Domain(corrupt("Launch revision overflow")))?;
    changed.updated_at = now.to_owned();
    changed
        .validate()
        .map_err(LaunchStoreError::from)
        .map_err(AttemptError::Domain)?;
    let next_mutation = current
        .mutation_version
        .checked_add(1)
        .ok_or_else(|| AttemptError::Domain(corrupt("Launch mutation version is exhausted")))?;
    let result = store
        .launch_data
        .launches
        .replace_one(
            doc! {
                "_id": id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": signed(current.launch.revision, "Launch revision")
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
            "Launch changed while claiming its work lease",
        ));
    }
    Ok(Some(StoredLaunch {
        launch: changed,
        mutation_version: next_mutation,
        home_capacity_admission: current.home_capacity_admission,
    }))
}

async fn reconcile(
    store: &MongoTriggerDefinitionStore,
    receipt: &StoredLaunch,
    at: mongodb::bson::Timestamp,
) -> Result<Launch, LaunchStoreError> {
    let mut session = transactions::snapshot_session(store, at).await?;
    let launch = queries::snapshot_launch(store, &receipt.launch.id, &mut session).await?;
    if launch.as_ref() == Some(receipt) {
        return Ok(receipt.launch.clone());
    }
    Err(transactions::still_unknown(
        "Launch lease claim",
        "the commit snapshot did not contain the exact claimed aggregate",
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
