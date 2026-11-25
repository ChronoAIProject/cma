//! Transactional operator ceilings and its activity evidence on one singleton.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::admission::{
    AdmissionCeilingsRead, AdmissionCeilingsState, AdmissionCeilingsStore,
};
use layer2_core::{StoreError, SubjectId};
use mongodb::bson::{doc, Bson, Document};
use mongodb::{Client, ClientSession, Collection};

use super::activity::{ActivityDoc, ACTIVITY};
use super::durability::majority_collection;
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, require_transaction_topology,
    start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
};
use super::schema::{self, DOCUMENT_SCHEMA_VERSION};
use super::settings::SETTINGS;

mod counts;
mod history;
#[cfg(test)]
mod mongo_tests;

pub struct MongoAdmissionCeilingsStore {
    initial_settings: super::settings_defaults::InitialSettings,
    client: Client,
    settings: Collection<Document>,
    activity: Collection<ActivityDoc>,
    database: mongodb::Database,
    deployment_limit: u16,
    pub(crate) pending_homes: super::pending_launch_homes::PendingLaunchHomes,
}

impl MongoAdmissionCeilingsStore {
    #[cfg(test)]
    pub(super) async fn open(
        client: &Client,
        database: &str,
        deployment_limit: u16,
    ) -> Result<Self, StoreError> {
        Self::open_with_defaults(client, database, deployment_limit, Default::default()).await
    }

    pub(super) async fn open_with_defaults(
        client: &Client,
        database: &str,
        deployment_limit: u16,
        initial_settings: super::settings_defaults::InitialSettings,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        require_transaction_topology(&database).await?;
        schema::initialize_collections(&database, &[SETTINGS, ACTIVITY]).await?;
        Ok(Self {
            pending_homes: super::pending_launch_homes::PendingLaunchHomes::new(
                client,
                database.name(),
            ),
            initial_settings,
            client: client.clone(),
            database: database.clone(),
            deployment_limit,
            settings: majority_collection(&database, SETTINGS),
            activity: majority_collection(&database, ACTIVITY),
        })
    }

    pub(super) fn with_activation(
        mut self,
        activation: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.pending_homes = self.pending_homes.with_activation(activation);
        self
    }

    pub(crate) fn with_pending_home_reconciliation(mut self, enabled: bool) -> Self {
        self.pending_homes = self.pending_homes.with_enabled(enabled);
        self
    }

    async fn apply(
        &self,
        expected_revision: u64,
        actor: &SubjectId,
        reason: &str,
        ceilings: layer2_core::admission::AdmissionCeilings,
        now: u64,
        session: &mut ClientSession,
    ) -> Result<AdmissionCeilingsState, ApplyError> {
        let document = self
            .settings
            .find_one(doc! {"_id": "platform"})
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        let current = document
            .as_ref()
            .map(from_document)
            .transpose()
            .map_err(ApplyError::Domain)?
            .unwrap_or_default();
        let Some(next) = current
            .changed(expected_revision, actor, reason, ceilings, now)
            .map_err(ApplyError::Domain)?
        else {
            return Ok(current);
        };
        let control = mongodb::bson::serialize_to_bson(&next).map_err(|_| {
            ApplyError::Domain(StoreError::Malformed("serialize admission ceilings".into()))
        })?;
        if document.is_some() {
            let revision_filter = if current.revision == 0 {
                doc! {"$or": [
                    {"admission_ceilings": {"$exists": false}},
                    {"admission_ceilings.revision": 0_i64},
                ]}
            } else {
                doc! {"admission_ceilings.revision": current.revision as i64}
            };
            let mut filter =
                doc! {"_id": "platform", "document_schema_version": DOCUMENT_SCHEMA_VERSION};
            filter.extend(revision_filter);
            let result = self
                .settings
                .update_one(filter, doc! {"$set": {"admission_ceilings": control}})
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?;
            if result.matched_count != 1 {
                return Err(ApplyError::Domain(unavailable(
                    "admission ceilings CAS did not match",
                )));
            }
        } else {
            let settings = mongodb::bson::serialize_to_bson(&self.initial_settings.platform())
                .map_err(|_| {
                    ApplyError::Domain(StoreError::Malformed("serialize default settings".into()))
                })?;
            self.settings
                .insert_one(doc! {
                    "_id": "platform", "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": 0_i64, "settings": settings, "admission_ceilings": control,
                })
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?;
        }
        let change = next
            .last_change
            .as_ref()
            .ok_or_else(|| ApplyError::Domain(unavailable("admission change evidence missing")))?;
        self.settings
            .insert_one(history::change_document(change).map_err(ApplyError::Domain)?)
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        let record = change.activity_record().map_err(ApplyError::Domain)?;
        self.activity
            .insert_one(ActivityDoc::from_record(&record))
            .session(session)
            .await
            .map_err(ApplyError::Driver)?;
        Ok(next)
    }
}

/// Parse the same settings document already read by every admission snapshot.
/// Missing additive state is open; corrupt or unsupported state fails closed.
pub(crate) fn from_document(document: &Document) -> Result<AdmissionCeilingsState, StoreError> {
    super::sandbox_mongo::require_version(document).map_err(StoreError::Malformed)?;
    let control = match document.get("admission_ceilings") {
        None => AdmissionCeilingsState::default(),
        Some(Bson::Document(value)) => mongodb::bson::deserialize_from_document(value.clone())
            .map_err(|_| StoreError::Malformed("stored admission ceilings is invalid".into()))?,
        Some(_) => {
            return Err(StoreError::Malformed(
                "stored admission ceilings is not a document".into(),
            ))
        }
    };
    control.validate()?;
    Ok(control)
}

enum ApplyError {
    Driver(mongodb::error::Error),
    Domain(StoreError),
}

#[async_trait::async_trait]
impl AdmissionCeilingsStore for MongoAdmissionCeilingsStore {
    async fn reconcile_pending_homes(
        &self,
        observed: u64,
        actor: &SubjectId,
        reason: &str,
        dry_run: bool,
        now: u64,
    ) -> Result<layer2_core::admission::PendingHomesReconciliation, StoreError> {
        self.pending_homes
            .correct(observed, actor, reason, dry_run, now)
            .await
    }

    async fn read(&self) -> Result<AdmissionCeilingsState, StoreError> {
        self.settings
            .find_one(doc! {"_id": "platform"})
            .await
            .map_err(|error| unavailable(&format!("read admission ceilings: {error}")))?
            .as_ref()
            .map(from_document)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    async fn read_with_history(&self) -> Result<AdmissionCeilingsRead, StoreError> {
        self.read_history_snapshot().await
    }

    async fn replace(
        &self,
        expected_revision: u64,
        actor: &SubjectId,
        reason: &str,
        ceilings: layer2_core::admission::AdmissionCeilings,
        now: u64,
    ) -> Result<AdmissionCeilingsState, StoreError> {
        for attempt in 0..TRANSACTION_ATTEMPTS {
            super::contention::backoff(attempt).await;
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|error| unavailable(&format!("start admission session: {error}")))?;
            match start_transaction(&mut session).await {
                Ok(()) => {}
                Err(error) if is_transient(&error) => continue,
                Err(error) => {
                    return Err(unavailable(&format!(
                        "start admission transaction: {error}"
                    )))
                }
            }
            let result = self
                .apply(
                    expected_revision,
                    actor,
                    reason,
                    ceilings.clone(),
                    now,
                    &mut session,
                )
                .await;
            match result {
                Ok(next) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(next),
                    CommitOutcome::Retry(_) => continue,
                    CommitOutcome::Unknown(error) => {
                        // Never rerun an uncertain transaction. A caller can
                        // read the durable state before a subsequent CAS.
                        return Err(unavailable(&format!(
                            "admission commit outcome is unknown: {error}"
                        )));
                    }
                    CommitOutcome::Failed(error) => {
                        return Err(unavailable(&format!("commit admission change: {error}")))
                    }
                },
                Err(ApplyError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    if is_transient(&error) || is_duplicate_key(&error) {
                        continue;
                    }
                    return Err(unavailable(&format!("write admission change: {error}")));
                }
                Err(ApplyError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
            }
        }
        Err(unavailable(
            "admission transaction contention exhausted the retry budget",
        ))
    }
}

fn unavailable(message: &str) -> StoreError {
    StoreError::Unavailable(message.into())
}
