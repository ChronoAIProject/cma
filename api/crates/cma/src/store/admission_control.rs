//! Transactional operator control and its activity evidence on one singleton.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::admission::{AdmissionControl, AdmissionControlRead, AdmissionControlStore};
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

mod history;

pub struct MongoAdmissionControlStore {
    initial_settings: super::settings_defaults::InitialSettings,
    client: Client,
    settings: Collection<Document>,
    activity: Collection<ActivityDoc>,
}

impl MongoAdmissionControlStore {
    #[cfg(test)]
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        Self::open_with_defaults(client, database, Default::default()).await
    }

    pub(super) async fn open_with_defaults(
        client: &Client,
        database: &str,
        initial_settings: super::settings_defaults::InitialSettings,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        require_transaction_topology(&database).await?;
        schema::initialize_collections(&database, &[SETTINGS, ACTIVITY]).await?;
        Ok(Self {
            initial_settings,
            client: client.clone(),
            settings: majority_collection(&database, SETTINGS),
            activity: majority_collection(&database, ACTIVITY),
        })
    }

    async fn apply(
        &self,
        expected_revision: u64,
        actor: &SubjectId,
        reason: &str,
        stopped: bool,
        now: u64,
        session: &mut ClientSession,
    ) -> Result<AdmissionControl, ApplyError> {
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
            .changed(expected_revision, actor, reason, stopped, now)
            .map_err(ApplyError::Domain)?
        else {
            return Ok(current);
        };
        let control = mongodb::bson::serialize_to_bson(&next).map_err(|_| {
            ApplyError::Domain(StoreError::Malformed("serialize admission control".into()))
        })?;
        if document.is_some() {
            let revision_filter = if current.revision == 0 {
                doc! {"$or": [
                    {"admission_control": {"$exists": false}},
                    {"admission_control.revision": 0_i64},
                ]}
            } else {
                doc! {"admission_control.revision": current.revision as i64}
            };
            let mut filter =
                doc! {"_id": "platform", "document_schema_version": DOCUMENT_SCHEMA_VERSION};
            filter.extend(revision_filter);
            let result = self
                .settings
                .update_one(filter, doc! {"$set": {"admission_control": control}})
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?;
            if result.matched_count != 1 {
                return Err(ApplyError::Domain(unavailable(
                    "admission control CAS did not match",
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
                    "revision": 0_i64, "settings": settings, "admission_control": control,
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
pub(crate) fn from_document(document: &Document) -> Result<AdmissionControl, StoreError> {
    super::sandbox_mongo::require_version(document).map_err(StoreError::Malformed)?;
    let control = match document.get("admission_control") {
        None => AdmissionControl::default(),
        Some(Bson::Document(value)) => mongodb::bson::deserialize_from_document(value.clone())
            .map_err(|_| StoreError::Malformed("stored admission control is invalid".into()))?,
        Some(_) => {
            return Err(StoreError::Malformed(
                "stored admission control is not a document".into(),
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
impl AdmissionControlStore for MongoAdmissionControlStore {
    async fn read(&self) -> Result<AdmissionControl, StoreError> {
        self.settings
            .find_one(doc! {"_id": "platform"})
            .await
            .map_err(|error| unavailable(&format!("read admission control: {error}")))?
            .as_ref()
            .map(from_document)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    async fn read_with_history(&self) -> Result<AdmissionControlRead, StoreError> {
        self.read_history_snapshot().await
    }

    async fn change(
        &self,
        expected_revision: u64,
        actor: &SubjectId,
        reason: &str,
        stopped: bool,
        now: u64,
    ) -> Result<AdmissionControl, StoreError> {
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
                .apply(expected_revision, actor, reason, stopped, now, &mut session)
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

#[cfg(test)]
mod mongo_tests;

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn missing_control_is_open_and_invalid_control_is_never_ignored() {
        assert_eq!(
            from_document(&doc! {"document_schema_version": 1_i64}).unwrap(),
            AdmissionControl::default()
        );
        for value in [
            Bson::Null,
            Bson::String("paused".into()),
            doc! {"revision": -1_i64}.into(),
            doc! {"revision": 1_i64}.into(),
        ] {
            assert!(from_document(
                &doc! {"document_schema_version": 1_i64, "admission_control": value}
            )
            .is_err());
        }
        assert!(from_document(&doc! {"document_schema_version": 2_i64}).is_err());
    }

    #[test]
    fn a_control_round_trip_preserves_the_exact_stop_evidence() {
        let stopped = AdmissionControl::default()
            .changed(
                0,
                &SubjectId::new("operator-a"),
                "contain incident",
                true,
                100,
            )
            .unwrap()
            .unwrap();
        let value = mongodb::bson::serialize_to_bson(&stopped).unwrap();
        assert_eq!(
            from_document(&doc! {"document_schema_version": 1_i64, "admission_control": value})
                .unwrap(),
            stopped
        );
    }
}
