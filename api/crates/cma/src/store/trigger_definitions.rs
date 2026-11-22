//! Versioned public-link Trigger definitions in MongoDB.
//!
//! This adapter owns definition authoring, immutable revisions, placements,
//! and publication guards. Launch runtime authority is a separate contract.

use std::sync::Arc;

use layer2_core::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};
use layer2_core::trigger_domain::{
    Placement, PlacementId, PlacementUpdate, PublicTrigger, PublicTriggerId, Trigger,
    TriggerAvailability, TriggerContent, TriggerError, TriggerId, TriggerPage, TriggerPageRequest,
    TriggerPreview, TriggerRevision, TriggerStats, TriggerStore, TriggerStoreError,
};
use layer2_core::StoreError;
use mongodb::bson::Document;
use mongodb::{Client, Collection};

use super::durability::majority_collection;
use super::sandbox_mongo::{require_transaction_topology, MongoProfileStore};
use super::schema::initialize_collections;

pub(in crate::store) use launches::decode_launch_aggregate;
pub(in crate::store) use launches::input_protocol::require_document as require_input_protocol_document;
pub(super) use launches::LAUNCHES;

pub(in crate::store) use launches::due_filter;
mod admission;
pub(in crate::store) use admission::{fence_new_agent_publisher, surviving_launch_revision};
pub(in crate::store) mod documents;
mod draft_admission;
mod indexes;
mod launches;
mod mutations;
mod profile_resolution;
mod publication;
mod publisher_erasure;
mod queries;

pub(super) const TRIGGER_DEFINITIONS: &str = "trigger_definitions";
pub(super) const TRIGGER_DEFINITION_REVISIONS: &str = "trigger_definition_revisions";
pub(super) const MUTATION_ATTEMPTS: usize = 8;

#[derive(Clone)]
pub struct MongoTriggerDefinitionStore {
    pub(super) initial_settings: super::settings_defaults::InitialSettings,
    pub(super) empty_discovery: super::empty_discovery::Policy,
    pub(super) client: Client,
    pub(super) definitions: Collection<Document>,
    pub(super) revisions: Collection<Document>,
    pub(super) profiles: Arc<MongoProfileStore>,
    pub(super) global_sandbox_limit: u16,
    pub(super) launch_admission_policy: layer2_core::trigger_domain::LaunchAdmissionPolicy,
    pub(super) launch_data: launches::LaunchCollections,
}

impl MongoTriggerDefinitionStore {
    pub(crate) fn with_initial_settings(
        mut self,
        initial: super::settings_defaults::InitialSettings,
    ) -> Self {
        self.initial_settings = initial;
        self
    }

    pub(crate) fn with_launch_admission_policy(
        mut self,
        policy: layer2_core::trigger_domain::LaunchAdmissionPolicy,
    ) -> Self {
        self.launch_admission_policy = policy;
        self
    }

    pub(crate) fn launch_admission_policy(
        &self,
    ) -> layer2_core::trigger_domain::LaunchAdmissionPolicy {
        self.launch_admission_policy
    }

    pub(crate) fn with_global_sandbox_limit(mut self, limit: u16) -> Self {
        self.global_sandbox_limit = limit;
        self
    }

    pub(super) async fn open(
        client: &Client,
        database_name: &str,
        profiles: Arc<MongoProfileStore>,
    ) -> Result<Self, StoreError> {
        let database = client.database(database_name);
        require_transaction_topology(&database).await?;
        initialize_collections(
            &database,
            &[TRIGGER_DEFINITIONS, TRIGGER_DEFINITION_REVISIONS],
        )
        .await?;
        let launch_data = launches::LaunchCollections::open(&database).await?;
        let store = Self {
            initial_settings: Default::default(),
            empty_discovery: Default::default(),
            client: client.clone(),
            definitions: majority_collection(&database, TRIGGER_DEFINITIONS),
            revisions: majority_collection(&database, TRIGGER_DEFINITION_REVISIONS),
            profiles,
            launch_data,
            global_sandbox_limit: 0,
            launch_admission_policy: Default::default(),
        };
        indexes::ensure(&store).await?;
        tracing::info!(
            database = database_name,
            "Trigger definition Mongo store ready"
        );
        Ok(store)
    }
}

#[async_trait::async_trait]
impl TriggerStore for MongoTriggerDefinitionStore {
    async fn create_trigger(&self, trigger: Trigger) -> Result<(), TriggerStoreError> {
        trigger.validate()?;
        if trigger.availability != TriggerAvailability::Draft
            || trigger.public_id.is_some()
            || !trigger.revisions.is_empty()
        {
            return Err(TriggerError::new("invalid_trigger_state", "availability").into());
        }
        draft_admission::create(self, &trigger).await
    }

    async fn trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
    ) -> Result<Option<Trigger>, TriggerStoreError> {
        Ok(queries::owned(self, id, owner)
            .await?
            .map(|stored| stored.trigger))
    }

    async fn delete_trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        archived_at: String,
    ) -> Result<(), TriggerStoreError> {
        mutations::delete(self, id, owner, archived_at).await
    }

    async fn trigger_revision(
        &self,
        id: &TriggerId,
        digest: &str,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        queries::revision(self, id, digest).await
    }

    async fn trigger_publisher_erased(
        &self,
        id: &TriggerId,
        digest: &str,
        public_id: &PublicTriggerId,
    ) -> Result<bool, TriggerStoreError> {
        publisher_erasure::prove(self, id, digest, public_id).await
    }

    async fn save_draft(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        expected: u64,
        draft: TriggerContent,
        updated_at: String,
    ) -> Result<Trigger, TriggerStoreError> {
        draft_admission::save(self, id, owner, expected, draft, updated_at).await
    }

    async fn publish_trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        expected: u64,
        published_at: String,
    ) -> Result<PublicTrigger, TriggerStoreError> {
        publication::publish(self, id, owner, expected, published_at).await
    }

    async fn trigger_by_public_id(
        &self,
        id: &PublicTriggerId,
    ) -> Result<Option<PublicTrigger>, TriggerStoreError> {
        queries::public(self, id).await
    }

    async fn preview_by_public_id(
        &self,
        id: &PublicTriggerId,
        publisher_display_name: String,
    ) -> Result<Option<TriggerPreview>, TriggerStoreError> {
        let Some(public) = queries::public(self, id).await? else {
            return Ok(None);
        };
        let profiles =
            profile_resolution::read_exact(self, &public.revision.content.target).await?;
        Ok(Some(TriggerPreview::project(
            public.availability,
            &public.revision,
            publisher_display_name,
            &profiles,
        )?))
    }

    async fn list_own_triggers(
        &self,
        owner: &SubjectId,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Trigger>, TriggerStoreError> {
        queries::list_owner(self, owner, page).await
    }

    async fn retire_trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        retired_at: String,
    ) -> Result<Trigger, TriggerStoreError> {
        mutations::owned(self, id, owner, |trigger| {
            trigger.retire(retired_at.clone())?;
            Ok(trigger.clone())
        })
        .await
    }

    async fn create_placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        source_url: String,
        label: String,
    ) -> Result<Placement, TriggerStoreError> {
        let placement = Placement::new(source_url, label)?;
        mutations::owned(self, trigger_id, owner, |trigger| {
            trigger.add_placement(placement.clone())?;
            Ok(placement.clone())
        })
        .await
    }

    async fn placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
    ) -> Result<Option<Placement>, TriggerStoreError> {
        Ok(queries::owned(self, trigger_id, owner)
            .await?
            .and_then(|stored| {
                stored
                    .trigger
                    .placements
                    .into_iter()
                    .find(|placement| &placement.id == placement_id)
            }))
    }

    async fn list_placements(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        include_retired: bool,
    ) -> Result<Vec<Placement>, TriggerStoreError> {
        let stored = queries::owned(self, trigger_id, owner)
            .await?
            .ok_or(TriggerStoreError::NotFound)?;
        Ok(stored
            .trigger
            .placements
            .into_iter()
            .filter(|placement| include_retired || placement.retired_at.is_none())
            .collect())
    }

    async fn update_placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
        update: PlacementUpdate,
    ) -> Result<Placement, TriggerStoreError> {
        mutations::owned(self, trigger_id, owner, |trigger| {
            trigger
                .update_placement(placement_id, update.clone())
                .map_err(Into::into)
        })
        .await
    }

    async fn retire_placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
        retired_at: String,
    ) -> Result<Placement, TriggerStoreError> {
        mutations::owned(self, trigger_id, owner, |trigger| {
            trigger
                .retire_placement(placement_id, retired_at.clone())
                .map_err(Into::into)
        })
        .await
    }

    async fn record_launch(&self, id: &TriggerId) -> Result<TriggerStats, TriggerStoreError> {
        mutations::record_launch(self, id).await
    }

    async fn record_agent_ready(&self, id: &TriggerId) -> Result<TriggerStats, TriggerStoreError> {
        mutations::record_ready(self, id).await
    }

    async fn guard_sandbox_profile_retirement(
        &self,
        id: &SandboxProfileId,
    ) -> Result<(), TriggerStoreError> {
        queries::guard(
            self,
            id.as_str(),
            "latest_sandbox_profile_id",
            "sandbox_profile_id",
            "record.revisions.content.target.sandbox_profile.id",
            |revision| &revision.content.target.sandbox_profile.id == id,
        )
        .await
    }

    async fn guard_workspace_profile_retirement(
        &self,
        id: &WorkspaceProfileId,
    ) -> Result<(), TriggerStoreError> {
        queries::guard(
            self,
            id.as_str(),
            "latest_workspace_profile_id",
            "workspace_profile_id",
            "record.revisions.content.target.workspace_profile.id",
            |revision| &revision.content.target.workspace_profile.id == id,
        )
        .await
    }

    async fn guard_agent_profile_retirement(
        &self,
        id: &AgentProfileId,
    ) -> Result<(), TriggerStoreError> {
        queries::guard(
            self,
            id.as_str(),
            "latest_agent_profile_id",
            "agent_profile_id",
            "record.revisions.content.target.agent_profile.id",
            |revision| &revision.content.target.agent_profile.id == id,
        )
        .await
    }
}

pub(super) fn driver_error(context: &str, error: mongodb::error::Error) -> TriggerStoreError {
    TriggerStoreError::Unavailable(format!("{context}: {error}"))
}

#[cfg(test)]
pub(in crate::store) mod integration_tests;
#[cfg(test)]
mod launch_mongo_integration_tests;
#[cfg(test)]
mod reference_integration_tests;

#[cfg(test)]
mod archive_mongo_tests;
