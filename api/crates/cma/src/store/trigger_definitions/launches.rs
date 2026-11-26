//! Durable Launch aggregates, ordered events, and service handoffs.

use layer2_core::ids::{LaunchId, PublicTriggerId, ServiceHandoffId, SubjectId};
use layer2_core::trigger_domain::{
    Launch, LaunchAdmission, LaunchAdmissionResult, LaunchCommit, LaunchEvent, LaunchPage,
    LaunchPageRequest, LaunchPlacementStats, LaunchStore, LaunchStoreError,
    ServiceConnectionHandoff, ServiceConnectionStore,
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
mod due;
pub(in crate::store) use due::filter as due_filter;
mod event_log;
mod handoffs;
mod indexes;
pub(super) mod input_protocol;
mod queries;
mod transactions;
mod validation;
mod visitor_quota;

pub(in crate::store) const LAUNCHES: &str = "trigger_launches";
pub(super) const LAUNCH_EVENTS: &str = "trigger_launch_events";
pub(super) const SERVICE_HANDOFFS: &str = "service_connection_handoffs";
pub(super) const INPUT_PROTOCOLS: &str = "trigger_launch_input_protocols";
pub(super) const TRANSACTION_ATTEMPTS: usize = 8;

#[derive(Clone)]
pub(in crate::store) struct LaunchCollections {
    visitor_days: Collection<Document>,
    settings: Collection<Document>,
    admissions: Collection<Document>,
    sandboxes: Collection<Document>,
    model_budget: Collection<Document>,
    launches: Collection<Document>,
    events: Collection<Document>,
    handoffs: Collection<Document>,
    input_protocols: Collection<Document>,
}

impl LaunchCollections {
    pub(super) async fn open(database: &Database) -> Result<Self, StoreError> {
        initialize_collections(
            database,
            &[
                LAUNCHES,
                LAUNCH_EVENTS,
                SERVICE_HANDOFFS,
                INPUT_PROTOCOLS,
                visitor_quota::COLLECTION,
            ],
        )
        .await?;
        let collections = Self {
            visitor_days: majority_collection(database, visitor_quota::COLLECTION),
            settings: majority_collection(database, "platform_settings"),
            admissions: majority_collection(database, "sandbox_admissions"),
            sandboxes: majority_collection(database, "sandboxes"),
            model_budget: majority_collection(database, crate::store::budget::COLLECTION),
            launches: majority_collection(database, LAUNCHES),
            events: majority_collection(database, LAUNCH_EVENTS),
            handoffs: majority_collection(database, SERVICE_HANDOFFS),
            input_protocols: majority_collection(database, INPUT_PROTOCOLS),
        };
        indexes::ensure(&collections).await?;
        visitor_quota::ensure_indexes(&collections).await?;
        Ok(collections)
    }
}

#[async_trait::async_trait]
impl LaunchStore for MongoTriggerDefinitionStore {
    async fn visitor_launches_admitted(
        &self,
        owner: &SubjectId,
        utc_day: u64,
    ) -> Result<u32, LaunchStoreError> {
        visitor_quota::admitted(&self.launch_data, owner, utc_day).await
    }

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

    async fn list_launches(
        &self,
        owner: &SubjectId,
        page: &LaunchPageRequest,
    ) -> Result<LaunchPage, LaunchStoreError> {
        queries::list_owner(self, owner, page).await
    }

    async fn source_launch(
        &self,
        owner: &SubjectId,
        agent_id: &layer2_core::AgentId,
        trigger: Option<&layer2_core::ids::TriggerId>,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        queries::source_launch(self, owner, agent_id, trigger).await
    }

    async fn placement_stats(
        &self,
        trigger_id: &layer2_core::trigger_domain::TriggerId,
    ) -> Result<Vec<LaunchPlacementStats>, LaunchStoreError> {
        queries::placement_stats(self, trigger_id).await
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
    async fn handoff_by_intent(
        &self,
        owner: &SubjectId,
        launch_id: &LaunchId,
        intent_id: &str,
    ) -> Result<Option<ServiceConnectionHandoff>, LaunchStoreError> {
        queries::handoff_by_intent(self, owner, launch_id, intent_id).await
    }

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
        | TriggerStoreError::AlreadyPublished
        | TriggerStoreError::Reference(_)
        | TriggerStoreError::ProfileReferenced { .. } => LaunchStoreError::TriggerUnavailable,
    }
}

pub(in crate::store) fn decode_launch_aggregate(
    document: &Document,
) -> Result<Launch, LaunchStoreError> {
    documents::read_launch(document).map(|stored| stored.launch)
}
