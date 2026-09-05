//! Durable Launch aggregates, ordered events, and service handoffs.

use layer2_core::ids::{LaunchId, PublicTriggerId, ServiceHandoffId, SubjectId};
use layer2_core::trigger_domain::{
    Launch, LaunchAdmission, LaunchAdmissionResult, LaunchCommit, LaunchEvent, LaunchStore,
    LaunchStoreError, ServiceConnectionHandoff, ServiceConnectionStore,
};
use layer2_core::StoreError;
use mongodb::bson::Document;
use mongodb::{Collection, Database};

use super::MongoTriggerDefinitionStore;
use crate::store::durability::majority_collection;
use crate::store::schema::initialize_collections;

mod admission;
mod claims;
mod commits;
mod counters;
mod documents;
mod event_log;
mod handoffs;
mod indexes;
mod queries;
mod transactions;
mod validation;

pub(super) const LAUNCHES: &str = "trigger_launches";
pub(super) const LAUNCH_EVENTS: &str = "trigger_launch_events";
pub(super) const SERVICE_HANDOFFS: &str = "service_connection_handoffs";
pub(super) const TRANSACTION_ATTEMPTS: usize = 8;

#[derive(Clone)]
pub(in crate::store) struct LaunchCollections {
    launches: Collection<Document>,
    events: Collection<Document>,
    handoffs: Collection<Document>,
}

impl LaunchCollections {
    pub(super) async fn open(database: &Database) -> Result<Self, StoreError> {
        initialize_collections(database, &[LAUNCHES, LAUNCH_EVENTS, SERVICE_HANDOFFS]).await?;
        let collections = Self {
            launches: majority_collection(database, LAUNCHES),
            events: majority_collection(database, LAUNCH_EVENTS),
            handoffs: majority_collection(database, SERVICE_HANDOFFS),
        };
        indexes::ensure(&collections).await?;
        Ok(collections)
    }
}

#[async_trait::async_trait]
impl LaunchStore for MongoTriggerDefinitionStore {
    async fn admit_launch(
        &self,
        admission: LaunchAdmission,
    ) -> Result<LaunchAdmissionResult, LaunchStoreError> {
        admission::admit(self, admission).await
    }

    async fn launch_by_command(
        &self,
        owner: &SubjectId,
        public_id: &PublicTriggerId,
        command_key_hash: &str,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        queries::by_command(self, owner, public_id, command_key_hash).await
    }

    async fn launch(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        queries::owned(self, id, owner).await
    }

    async fn commit_launch(&self, commit: LaunchCommit) -> Result<Launch, LaunchStoreError> {
        commits::commit(self, commit).await
    }

    async fn launch_events(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<LaunchEvent>, LaunchStoreError> {
        event_log::events(self, id, owner, after, limit).await
    }

    async fn due_launches(&self, now: &str, limit: u16) -> Result<Vec<LaunchId>, LaunchStoreError> {
        queries::due(self, now, limit).await
    }

    async fn claim_launch(
        &self,
        id: &LaunchId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        claims::claim(self, id, holder, now, lease_expires_at).await
    }
}

#[async_trait::async_trait]
impl ServiceConnectionStore for MongoTriggerDefinitionStore {
    async fn create_handoff(
        &self,
        handoff: ServiceConnectionHandoff,
    ) -> Result<ServiceConnectionHandoff, LaunchStoreError> {
        handoffs::create(self, handoff).await
    }

    async fn handoff(
        &self,
        id: &ServiceHandoffId,
        owner: &SubjectId,
    ) -> Result<Option<ServiceConnectionHandoff>, LaunchStoreError> {
        handoffs::owned(self, id, owner).await
    }

    async fn save_handoff(
        &self,
        expected_revision: u64,
        handoff: ServiceConnectionHandoff,
    ) -> Result<ServiceConnectionHandoff, LaunchStoreError> {
        handoffs::save(self, expected_revision, handoff).await
    }
}

pub(super) fn unavailable(error: impl std::fmt::Display) -> LaunchStoreError {
    LaunchStoreError::Unavailable(error.to_string())
}

pub(super) fn corrupt(detail: impl Into<String>) -> LaunchStoreError {
    LaunchStoreError::Corrupt(detail.into())
}

pub(super) fn trigger_error(
    error: layer2_core::trigger_domain::TriggerStoreError,
) -> LaunchStoreError {
    use layer2_core::trigger_domain::TriggerStoreError;

    match error {
        TriggerStoreError::Unavailable(detail) => LaunchStoreError::Unavailable(detail),
        TriggerStoreError::Corrupt(detail) => LaunchStoreError::Corrupt(detail),
        TriggerStoreError::Validation(error) => LaunchStoreError::Validation(error),
        TriggerStoreError::NotFound
        | TriggerStoreError::Duplicate
        | TriggerStoreError::Conflict { .. }
        | TriggerStoreError::Reference(_)
        | TriggerStoreError::ProfileReferenced { .. } => LaunchStoreError::TriggerUnavailable,
    }
}
