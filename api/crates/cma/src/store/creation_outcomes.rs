//! Durable creation evidence and the write fence shared with Sandbox admission.

use std::time::Duration;

use axum::http::StatusCode;
use layer1_public::rest::creation_outcomes::{
    self as contract, Backend, Begin, Identity, LookupRequest, Operation, Outcome, RefusalCode,
    ValidationRefusal,
};
use layer2_core::{sandbox::store::CreationFence, StoreError, SubjectId};
use mongodb::{
    bson::{doc, DateTime, Document},
    options::{IndexOptions, ReturnDocument},
    ClientSession, Collection, Database, IndexModel,
};
use serde::{Deserialize, Serialize};

use super::{durability::majority_collection, sandbox_mongo as transaction, schema};
use crate::{config::CreationOutcomesConfig, store::transaction_terminal::GuardedTerminal};

mod coverage;
mod expiry;
mod legacy;
mod publication;
mod reads;
mod validation;
pub(super) use publication::{
    cancel_with_operation, cancellation_committed, link_operation, publish_sandbox,
    refuse_with_operation,
};

pub(super) const COLLECTION: &str = "sandbox_creation_proofs";
const VERSION: i64 = schema::DOCUMENT_SCHEMA_VERSION;
const IO_BUDGET: Duration = Duration::from_millis(750);

#[derive(Clone)]
pub(crate) struct CreationOutcomes {
    database: Database,
    config: CreationOutcomesConfig,
    records: Collection<Document>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    #[serde(rename = "_id")]
    record_id: String,
    #[serde(rename = "document_schema_version")]
    version: i64,
    owner: String,
    deployment: String,
    operation: Operation,
    epoch: String,
    day: i64,
    identity: String,
    generation: String,
    lease_until: DateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    state: Evidence,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Evidence {
    Pending,
    Created { sandbox_id: String },
    Refused { code: RefusalCode },
}

impl CreationOutcomes {
    pub(crate) async fn open(
        database: Database,
        config: CreationOutcomesConfig,
    ) -> Result<Self, StoreError> {
        schema::initialize_collections(&database, &[COLLECTION]).await?;
        let records = majority_collection(&database, COLLECTION);
        records
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "owner": 1, "_id": 1 })
                    .options(
                        IndexOptions::builder()
                            .name("creation_proof_owner".to_owned())
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(driver)?;
        records
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"epoch":1,"state.kind":1,"lease_until":1})
                    .options(
                        IndexOptions::builder()
                            .name("creation_proof_pending_lease".to_owned())
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(driver)?;
        let store = Self {
            database,
            config,
            records,
        };
        store.initialize_coverage().await?;
        tracing::info!(
            terminal_settlement_enabled = store.config.terminal_settlement,
            "Sandbox creation outcome coverage initialized"
        );
        Ok(store)
    }

    async fn read_record(
        records: &Collection<Document>,
        identity: &Identity<'_>,
        session: &mut ClientSession,
    ) -> Result<Option<Record>, StatusCode> {
        let row = records
            .find_one(doc! {
                "_id": record_id(identity), "owner": identity.owner.as_str(),
            })
            .max_time(IO_BUDGET)
            .session(session)
            .await
            .map_err(unavailable)?;
        let Some(row) = row else { return Ok(None) };
        let row: Record = mongodb::bson::deserialize_from_document(row).map_err(unavailable)?;
        let period = identity
            .period()
            .map_err(identity_error)?
            .ok_or(StatusCode::CONFLICT)?;
        if row.version != VERSION
            || row.deployment != identity.deployment
            || row.operation != identity.operation
            || row.epoch != period.epoch
            || row.day != period.day
            || !contract::lower_hex(&row.generation, 32)
            || row
                .operation_id
                .as_ref()
                .is_some_and(|id| !contract::lower_hex(id, 32))
        {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        if row.identity != identity.digest() {
            return Err(StatusCode::CONFLICT);
        }
        Ok(Some(row))
    }

    async fn begin_transaction(
        &self,
        identity: &Identity<'_>,
        session: &mut ClientSession,
    ) -> Result<Begin, StartError> {
        let period = identity
            .period()
            .map_err(identity_error)?
            .ok_or(StatusCode::BAD_REQUEST)?;
        if period.epoch != self.config.epoch {
            return Ok(Begin::Closed);
        }
        if let Some(row) = Self::read_record(&self.records, identity, session).await? {
            return Ok(Begin::Existing {
                operation_id: row.operation_id,
                cached_response: None,
            });
        }
        if super::account_deletions::fence_subject_in_session(
            &self.database,
            identity.owner,
            session,
        )
        .await?
        {
            return Err(StatusCode::FORBIDDEN.into());
        }
        if !reads::owner_day_covered(&self.database, identity.owner, period.day, session).await? {
            return Ok(Begin::Closed);
        }
        let Some(clock) = self.admit_day(period.day, session).await? else {
            return Ok(Begin::Closed);
        };
        let generation = uuid::Uuid::new_v4().simple().to_string();
        let record_id = record_id(identity);
        let result = CreationFence {
            record_id: record_id.clone(),
            generation: generation.clone(),
            epoch: period.epoch.to_owned(),
            lifecycle_lease: None,
        };
        let lease_until = DateTime::from_millis(clock.timestamp_millis() + 120_000);
        self.records
            .insert_one(
                mongodb::bson::serialize_to_document(&Record {
                    record_id,
                    version: VERSION,
                    owner: identity.owner.to_string(),
                    deployment: self.config.deployment.clone(),
                    operation: identity.operation,
                    epoch: period.epoch.to_owned(),
                    day: period.day,
                    identity: identity.digest(),
                    generation,
                    lease_until,
                    operation_id: None,
                    state: Evidence::Pending,
                })
                .map_err(unavailable)?,
            )
            .session(session)
            .await?;
        Ok(Begin::Acquired(result))
    }
}

#[async_trait::async_trait]
impl Backend for CreationOutcomes {
    fn deployment(&self) -> &str {
        &self.config.deployment
    }

    fn terminal_settlement_enabled(&self) -> bool {
        self.config.terminal_settlement
    }

    fn validation_settlement_enabled(&self) -> bool {
        self.config.terminal_settlement && self.config.validation_settlement
    }

    async fn refuse_validation(
        &self,
        identity: &Identity<'_>,
        refusal: ValidationRefusal,
    ) -> Result<(), StatusCode> {
        self.settle_validation(identity, refusal).await
    }

    async fn lookup(
        &self,
        identity: &Identity<'_>,
        query: &LookupRequest,
    ) -> Result<Outcome, StatusCode> {
        let start = std::time::Instant::now();
        let result = self.resolve(identity, query).await;
        let unknown_reason = match &result {
            Ok(Outcome::Unknown { reason }) => Some(reason),
            _ => None,
        };
        tracing::debug!(
            creation_proof = %record_id(identity),
            operation = ?identity.operation,
            outcome = reads::outcome_name(&result),
            ?unknown_reason,
            elapsed_ms = start.elapsed().as_millis(),
            "Sandbox creation lookup"
        );
        result
    }

    async fn begin(&self, identity: &Identity<'_>) -> Result<Begin, StatusCode> {
        for _ in 0..4 {
            let mut session = self
                .database
                .client()
                .start_session()
                .await
                .map_err(unavailable)?;
            transaction::start_transaction(&mut session)
                .await
                .map_err(unavailable)?;
            let result = match self.begin_transaction(identity, &mut session).await {
                Ok(result) => result,
                Err(StartError::Driver(error)) if transaction::is_transient(&error) => {
                    session.abort_guarded().await.map_err(unavailable)?;
                    continue;
                }
                Err(error) => {
                    session.abort_guarded().await.map_err(unavailable)?;
                    return Err(error.status());
                }
            };
            match transaction::commit_transaction(&mut session).await {
                transaction::CommitOutcome::Committed => {
                    return self.recover_cached(identity, result).await;
                }
                transaction::CommitOutcome::Retry(_) => {}
                // Never give an execution permit after an ambiguous acknowledgement.
                transaction::CommitOutcome::Unknown(_) | transaction::CommitOutcome::Failed(_) => {
                    return Err(StatusCode::SERVICE_UNAVAILABLE)
                }
            }
        }
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }

    async fn refuse_direct(
        &self,
        owner: &SubjectId,
        fence: &CreationFence,
        code: RefusalCode,
    ) -> Result<bool, StatusCode> {
        publication::refuse_direct(&self.database, owner, fence, code)
            .await
            .map_err(unavailable)
    }
}

enum StartError {
    Http(StatusCode),
    Driver(mongodb::error::Error),
}

impl From<StatusCode> for StartError {
    fn from(status: StatusCode) -> Self {
        Self::Http(status)
    }
}
impl From<mongodb::error::Error> for StartError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}
impl StartError {
    fn status(self) -> StatusCode {
        match self {
            Self::Http(status) => status,
            Self::Driver(error) => unavailable(error),
        }
    }
}

fn record_id(identity: &Identity<'_>) -> String {
    contract::hash_fields(&[
        "cma-creation-record-v1",
        identity.deployment,
        identity.owner.as_str(),
        identity.operation.path(),
        identity.key,
    ])
}

fn identity_error(error: contract::IdentityError) -> StatusCode {
    match error {
        contract::IdentityError::Invalid => StatusCode::BAD_REQUEST,
        contract::IdentityError::Mismatch => StatusCode::CONFLICT,
    }
}

fn unavailable<E>(_: E) -> StatusCode {
    StatusCode::SERVICE_UNAVAILABLE
}

fn driver(_: mongodb::error::Error) -> StoreError {
    StoreError::Unavailable("Sandbox creation evidence store is unavailable".into())
}

#[cfg(test)]
mod mongo_tests;
