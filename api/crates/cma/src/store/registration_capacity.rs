//! Public registration receipts and their transaction-fenced capacity counter.

use crate::store::transaction_terminal::GuardedTerminal as _;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use layer2_core::{StoreError, SubjectId};
use mongodb::{
    bson::{doc, Bson, Document},
    Client, ClientSession, Collection, Database,
};

use crate::auth::registration::RegistrationCapacity;

use super::{
    durability::majority_collection,
    sandbox_mongo::{
        commit_transaction, is_duplicate_key, is_transient, require_transaction_topology,
        start_transaction, CommitOutcome,
    },
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};

const ATTEMPTS: usize = 16;
const QUERY_TIME: Duration = Duration::from_secs(3);
const ADMISSIONS: &str = "sandbox_admissions";
const SETTINGS: &str = "platform_settings";

pub(crate) mod activation;

// Retained identity and ownership evidence is sufficient; a resource need not
// still be live. Do not decrypt credentials merely to recognize an owner.
const EXISTING_EVIDENCE: &[(&str, &str)] = &[
    ("users", "_id"),
    ("auth_subjects", "_id"),
    ("auth_api_bearers", "subject"),
    ("sandboxes", "owner"),
    ("agents", "owner"),
    ("sandbox_profiles", "owner"),
    ("workspace_profiles", "owner"),
    ("agent_profiles", "owner"),
    ("event_worker_profiles", "owner"),
    ("trigger_definitions", "owner"),
    ("trigger_launches", "owner"),
    ("bots", "owner"),
    ("provider_bindings", "record.owner"),
    ("cma_document_artifacts", "owner_subject"),
];

pub struct MongoRegistrationCapacityStore {
    initial_settings: super::settings_defaults::InitialSettings,
    client: Client,
    database: Database,
    admissions: Collection<Document>,
    settings: Collection<Document>,
    activated: std::sync::Arc<AtomicBool>,
}

impl MongoRegistrationCapacityStore {
    pub(crate) fn activation_observation(&self) -> std::sync::Arc<AtomicBool> {
        self.activated.clone()
    }

    #[cfg(test)]
    pub(crate) async fn open(
        client: &Client,
        database: &str,
        writes_enabled: bool,
    ) -> Result<Self, StoreError> {
        Self::open_with_defaults(client, database, writes_enabled, Default::default()).await
    }

    /// Activation establishes the cohort boundary once and survives gate closure.
    pub(crate) async fn open_with_defaults(
        client: &Client,
        database: &str,
        writes_enabled: bool,
        initial_settings: super::settings_defaults::InitialSettings,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        require_transaction_topology(&database).await?;
        schema::initialize_collections(&database, &[ADMISSIONS, SETTINGS]).await?;
        let admissions: Collection<Document> = majority_collection(&database, ADMISSIONS);
        if writes_enabled {
            activation::activate(
                client,
                &admissions,
                &majority_collection(&database, SETTINGS),
                initial_settings,
            )
            .await?;
        }
        let marker = admissions
            .find_one(capacity_key())
            .await
            .map_err(|error| unavailable(&format!("read registration activation: {error}")))?;
        if let Some(marker) = &marker {
            registration_count(marker)?;
        }
        Ok(Self {
            initial_settings,
            client: client.clone(),
            settings: majority_collection(&database, SETTINGS),
            database,
            admissions,
            activated: std::sync::Arc::new(AtomicBool::new(marker.is_some())),
        })
    }

    async fn known_subject(
        &self,
        subject: &SubjectId,
        mut session: Option<&mut ClientSession>,
    ) -> Result<bool, mongodb::error::Error> {
        // A public receipt wins over the auth/directory rows its first callback
        // subsequently writes. It remains an exact, counted-once replay.
        let query = self.admissions.find_one(registration_key(subject));
        let registered = match session.as_deref_mut() {
            Some(session) => query.session(session).await?,
            None => query.await?,
        };
        if registered.is_some() {
            return Ok(true);
        }
        for (collection, field) in EXISTING_EVIDENCE {
            let mut filter = Document::new();
            filter.insert(*field, subject.as_str());
            let rows = self.database.collection::<Document>(collection);
            let query = rows
                .find_one(filter)
                .projection(doc! {"_id":1})
                .max_time(QUERY_TIME);
            let existing = match session.as_deref_mut() {
                Some(session) => query.session(session).await?,
                None => query.await?,
            };
            if existing.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn limit(
        &self,
        session: Option<&mut ClientSession>,
    ) -> Result<Option<u32>, AttemptError> {
        let query = self.settings.find_one(doc! {"_id":"platform"});
        let document = match session {
            Some(session) => query.session(session).await,
            None => query.await,
        }
        .map_err(AttemptError::Driver)?;
        document
            .as_ref()
            .map(super::admission_ceilings::from_document)
            .transpose()
            .map(|state| state.unwrap_or_default().ceilings.registrations)
            .map_err(|_| AttemptError::Unreadable)
    }

    async fn apply(
        &self,
        subject: &SubjectId,
        session: &mut ClientSession,
    ) -> Result<bool, AttemptError> {
        if self
            .known_subject(subject, Some(&mut *session))
            .await
            .map_err(AttemptError::Driver)?
        {
            return Ok(true);
        }
        let protected =
            super::settings_barrier::acquire(&self.settings, self.initial_settings, session)
                .await
                .map_err(|error| match error {
                    super::settings_barrier::Error::Driver(error) => AttemptError::Driver(error),
                    super::settings_barrier::Error::Unreadable => AttemptError::Unreadable,
                })?;
        let limit = super::admission_ceilings::from_document(protected.document())
            .map_err(|_| AttemptError::Unreadable)?
            .ceilings
            .registrations;
        let active = activation::read(&self.admissions, Some(&mut *session))
            .await
            .map_err(|error| match error {
                activation::Error::Driver(error) => AttemptError::Driver(error),
                activation::Error::Unreadable(_) => AttemptError::Unreadable,
            })?;
        if !active {
            return Ok(limit.is_none());
        }
        self.activated.store(true, Ordering::Relaxed);
        // The counter is O(1) and lives in the fence itself. Every successful
        // first registration, including unlimited admission, increments it.
        let mut filter = capacity_key();
        filter.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION);
        filter.insert("activated_at", doc! {"$type":["int","long"],"$gte":0_i64});
        filter.insert(
            "count",
            match limit {
                Some(limit) => Bson::Document(
                    doc! {"$type":["int","long"],"$gte":0_i64,"$lt":i64::from(limit)},
                ),
                None => Bson::Document(doc! {"$type":["int","long"],"$gte":0_i64,"$lt":i64::MAX}),
            },
        );
        let reserved = self
            .admissions
            .update_one(filter, doc! {"$inc":{"count":1_i64}})
            .session(&mut *session)
            .await
            .map_err(AttemptError::Driver)?;
        if reserved.matched_count != 1 {
            return Ok(false);
        }
        let registered_at = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339())
            .and_then(|now| i64::try_from(now).ok())
            .ok_or(AttemptError::Unreadable)?;
        let mut receipt = registration_key(subject);
        receipt.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION);
        receipt.insert("registered_at", registered_at);
        self.admissions
            .insert_one(receipt)
            .session(session)
            .await
            .map_err(AttemptError::Driver)?;
        Ok(true)
    }
}

#[async_trait::async_trait]
impl RegistrationCapacity for MongoRegistrationCapacityStore {
    async fn existing_subject(&self, subject: &SubjectId) -> Result<bool, StoreError> {
        self.known_subject(subject, None)
            .await
            .map_err(|error| unavailable(&format!("read existing consumer: {error}")))
    }

    async fn admit(&self, subject: &SubjectId) -> Result<bool, StoreError> {
        for attempt in 0..ATTEMPTS {
            super::contention::backoff(attempt).await;
            let mut session =
                self.client.start_session().await.map_err(|error| {
                    unavailable(&format!("start registration session: {error}"))
                })?;
            match start_transaction(&mut session).await {
                Ok(()) => {}
                Err(error) if is_transient(&error) => continue,
                Err(error) => {
                    return Err(unavailable(&format!(
                        "start registration transaction: {error}"
                    )))
                }
            }
            match self.apply(subject, &mut session).await {
                Ok(accepted) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(accepted),
                    CommitOutcome::Retry(_) => continue,
                    CommitOutcome::Unknown(error) => {
                        return Err(unavailable(&format!(
                            "registration commit outcome is unknown: {error}"
                        )))
                    }
                    CommitOutcome::Failed(error) => {
                        return Err(unavailable(&format!("commit registration: {error}")))
                    }
                },
                Err(AttemptError::Unreadable) => {
                    let _ = session.abort_guarded().await;
                    return Ok(false);
                }
                Err(AttemptError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    if is_transient(&error) || is_duplicate_key(&error) {
                        continue;
                    }
                    return Err(unavailable(&format!("reserve registration: {error}")));
                }
            }
        }
        tracing::warn!("registration contention exhausted the retry budget");
        Ok(false)
    }

    async fn available(&self) -> Result<bool, StoreError> {
        let limit = match self.limit(None).await {
            Ok(limit) => Ok(limit),
            Err(AttemptError::Unreadable) => return Ok(false),
            Err(AttemptError::Driver(error)) => {
                return Err(unavailable(&format!("read registration ceiling: {error}")))
            }
        };
        self.available_with_limit(limit).await
    }

    async fn available_with_limit(
        &self,
        limit: Result<Option<u32>, ()>,
    ) -> Result<bool, StoreError> {
        let limit = match limit {
            Ok(None) => return Ok(true),
            Ok(Some(limit)) => limit,
            Err(()) => return Ok(false),
        };
        let row = self
            .admissions
            .find_one(capacity_key())
            .await
            .map_err(|error| unavailable(&format!("read registration count: {error}")))?;
        let count = row.as_ref().map(registration_count).transpose()?;
        if count.is_some() {
            self.activated.store(true, Ordering::Relaxed);
        }
        Ok(count.is_some_and(|count| count < u64::from(limit)))
    }

    async fn admit_with_limit(
        &self,
        subject: &SubjectId,
        limit: Result<Option<u32>, ()>,
    ) -> Result<bool, StoreError> {
        match limit {
            Err(()) => Ok(false),
            _ => self.admit(subject).await,
        }
    }
}

enum AttemptError {
    Driver(mongodb::error::Error),
    Unreadable,
}

fn capacity_key() -> Document {
    doc! {"_id":{"scope":"registration_capacity"}}
}

fn registration_key(subject: &SubjectId) -> Document {
    doc! {"_id":{"scope":"registration","subject":subject.as_str()}}
}

pub(crate) fn registration_count(document: &Document) -> Result<u64, StoreError> {
    super::sandbox_mongo::require_version(document).map_err(StoreError::Malformed)?;
    let unsigned = |field| match document.get(field) {
        Some(Bson::Int32(value)) => u64::try_from(*value).ok(),
        Some(Bson::Int64(value)) => u64::try_from(*value).ok(),
        _ => None,
    };
    unsigned("activated_at").ok_or_else(|| unavailable("registration activation is invalid"))?;
    unsigned("count").ok_or_else(|| unavailable("registration count is invalid"))
}

#[cfg(test)]
fn read_count(document: &Document) -> Result<u64, StoreError> {
    registration_count(document)
}

/// Presence is conservative even for an unreadable receipt: it cannot grant
/// access to the reserve by masquerading as an existing consumer.
pub(crate) async fn is_public(
    admissions: &Collection<Document>,
    owner: &SubjectId,
    session: Option<&mut ClientSession>,
) -> Result<bool, mongodb::error::Error> {
    let query = admissions
        .find_one(registration_key(owner))
        .projection(doc! {"_id":1});
    let receipt = match session {
        Some(session) => query.session(session).await?,
        None => query.await?,
    };
    Ok(receipt.is_some())
}

fn unavailable(message: &str) -> StoreError {
    StoreError::Unavailable(message.into())
}

#[cfg(test)]
mod mongo_tests;
