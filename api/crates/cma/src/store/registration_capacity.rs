//! Public registration receipts and their transaction-fenced capacity counter.

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
    client: Client,
    database: Database,
    admissions: Collection<Document>,
    settings: Collection<Document>,
    activated: AtomicBool,
}

impl MongoRegistrationCapacityStore {
    /// The first open gate establishes the cohort boundary once. Later gate
    /// closure retains tracking, so new sign-ins cannot become existing consumers.
    pub(crate) async fn open(
        client: &Client,
        database: &str,
        writes_enabled: bool,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        require_transaction_topology(&database).await?;
        schema::initialize_collections(&database, &[ADMISSIONS, SETTINGS]).await?;
        let admissions: Collection<Document> = majority_collection(&database, ADMISSIONS);
        if writes_enabled {
            let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339())
                .and_then(|now| i64::try_from(now).ok())
                .ok_or_else(|| unavailable("registration activation clock is unavailable"))?;
            match admissions
                .update_one(
                    capacity_key(),
                    doc! {"$setOnInsert": {
                        "document_schema_version":DOCUMENT_SCHEMA_VERSION,
                        "count":0_i64,"activated_at":now,
                    }},
                )
                .upsert(true)
                .await
            {
                Ok(_) => {}
                Err(error) if is_duplicate_key(&error) => {}
                Err(error) => return Err(unavailable(&format!("activate registration: {error}"))),
            }
        }
        let marker = admissions
            .find_one(capacity_key())
            .await
            .map_err(|error| unavailable(&format!("read registration activation: {error}")))?;
        if let Some(marker) = &marker {
            registration_count(marker)?;
        }
        Ok(Self {
            client: client.clone(),
            settings: majority_collection(&database, SETTINGS),
            database,
            admissions,
            activated: AtomicBool::new(marker.is_some()),
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
        let limit = self.limit(Some(&mut *session)).await?;
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
    async fn tracking_active(&self) -> Result<bool, StoreError> {
        if self.activated.load(Ordering::Relaxed) {
            return Ok(true);
        }
        let active = activation::read(&self.admissions, None)
            .await
            .map_err(activation::Error::into_store_error)?;
        if active {
            self.activated.store(true, Ordering::Relaxed);
        }
        Ok(active)
    }

    async fn existing_subject(&self, subject: &SubjectId) -> Result<bool, StoreError> {
        self.known_subject(subject, None)
            .await
            .map_err(|error| unavailable(&format!("read existing consumer: {error}")))
    }

    async fn admit(&self, subject: &SubjectId) -> Result<bool, StoreError> {
        if !self.tracking_active().await? {
            // No cohort rows are written before activation. A retained ceiling
            // without its activation fence cannot safely admit new registrations.
            return match self.limit(None).await {
                Ok(None) => Ok(true),
                Ok(Some(_)) | Err(AttemptError::Unreadable) => Ok(false),
                Err(AttemptError::Driver(error)) => {
                    Err(unavailable(&format!("read registration ceiling: {error}")))
                }
            };
        }
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
                    let _ = session.abort_transaction().await;
                    return Ok(false);
                }
                Err(AttemptError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
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
            Ok(None) if !self.tracking_active().await? => Ok(true),
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
    session: &mut ClientSession,
) -> Result<bool, mongodb::error::Error> {
    admissions
        .find_one(registration_key(owner))
        .projection(doc! {"_id":1})
        .session(session)
        .await
        .map(|row| row.is_some())
}

fn unavailable(message: &str) -> StoreError {
    StoreError::Unavailable(message.into())
}

#[cfg(test)]
mod mongo_tests;
