//! Deterministic deployment-owned profiles created on a fresh database.

use layer2_core::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileRef, AgentProfileRevision, AgentProvider,
    ModelAccess, PermissionLevel, ProfileCapacityBounds, ProfileStoreError, ProfileVisibility,
    SandboxProfile, SandboxProfileContent, SandboxProfileRevision, ThinkingEffort,
    WorkspaceLocation, WorkspaceProfile, WorkspaceProfileContent, WorkspaceProfileRef,
    WorkspaceProfileRevision,
};
use layer2_core::{
    AgentProfileId, SandboxProfileId, StoreError, SubjectId, WorkspaceProfileId,
    DEFAULT_AGENT_PROFILE_ID, DEFAULT_PROFILE_REVISION, DEFAULT_SANDBOX_PROFILE_ID,
    DEFAULT_WORKSPACE_PROFILE_ID,
};
use mongodb::bson::{doc, Bson, Document};
use mongodb::Collection;
use serde::de::DeserializeOwned;

use super::profile_documents::revision_document;
use super::sandbox_mongo::{read_record, require_version, MongoProfileStore};
use super::{agent_profiles, sandbox_profiles, workspace_profiles};

pub(super) const SYSTEM_OWNER: &str = "system";
const SEEDED_AT: &str = "2026-09-05T00:00:00Z";
const DEFAULT_MAX_WORKSPACES: u8 = 4;
const DEFAULT_MAX_AGENTS_PER_WORKSPACE: u8 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DefaultProfileSpec {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub agent_model: String,
    pub agent_effort: ThinkingEffort,
}

struct DefaultProfileRecords {
    agent: AgentProfile,
    agent_revision: AgentProfileRevision,
    workspace: WorkspaceProfile,
    workspace_revision: WorkspaceProfileRevision,
    sandbox: SandboxProfile,
    sandbox_revision: SandboxProfileRevision,
}

impl DefaultProfileSpec {
    async fn records(
        &self,
        bounds: ProfileCapacityBounds,
        scanner: &dyn layer2_core::profiles::PublicValueScanner,
    ) -> Result<DefaultProfileRecords, StoreError> {
        let owner = SubjectId::new(SYSTEM_OWNER);
        let agent = AgentProfile {
            internal: false,
            id: AgentProfileId(DEFAULT_AGENT_PROFILE_ID.into()),
            owner: owner.clone(),
            name: "Default agent".into(),
            description: "Deployment-owned default agent profile.".into(),
            visibility: ProfileVisibility::Public,
            forked_from: None,
            uses_count: 0,
            draft: AgentProfileContent {
                isolated_agent: false,
                model: Some(self.agent_model.clone()),
                thinking_effort: self.agent_effort,
                permission_level: PermissionLevel::Autonomous,
                skills: Vec::new(),
                system_prompt: String::new(),
            },
            draft_version: 1,
            created_at: SEEDED_AT.into(),
            updated_at: SEEDED_AT.into(),
            retired_at: None,
        };
        let agent_revision = agent
            .publish(DEFAULT_PROFILE_REVISION, SEEDED_AT.into())
            .map_err(profile_invalid)?;
        let workspace = WorkspaceProfile {
            internal: false,
            id: WorkspaceProfileId(DEFAULT_WORKSPACE_PROFILE_ID.into()),
            owner: owner.clone(),
            name: "Default workspace".into(),
            description: "Deployment-owned default workspace profile.".into(),
            visibility: ProfileVisibility::Public,
            forked_from: None,
            uses_count: 0,
            draft: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "workspace".into(),
                },
                name: "workspace".into(),
                setup_commands: Vec::new(),
                agent_profiles: vec![AgentProfileRef::new(
                    agent.id.clone(),
                    DEFAULT_PROFILE_REVISION,
                )],
            },
            draft_version: 1,
            created_at: SEEDED_AT.into(),
            updated_at: SEEDED_AT.into(),
            retired_at: None,
        };
        let workspace_revision = workspace
            .publish(DEFAULT_PROFILE_REVISION, SEEDED_AT.into())
            .map_err(profile_invalid)?;
        let sandbox = SandboxProfile {
            internal: false,
            availability: Default::default(),
            id: SandboxProfileId(DEFAULT_SANDBOX_PROFILE_ID.into()),
            owner,
            name: "Default sandbox".into(),
            description: "Deployment-owned default sandbox profile.".into(),
            visibility: ProfileVisibility::Public,
            forked_from: None,
            uses_count: 0,
            draft: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: self.cpu_millis,
                memory_mib: self.memory_mib,
                nyxid_services: crate::config::REQUIRED_BASELINE_SERVICES
                    .iter()
                    .map(|id| layer2_core::ServiceId::new(*id))
                    .collect(),
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: DEFAULT_MAX_WORKSPACES,
                max_agents_per_workspace: DEFAULT_MAX_AGENTS_PER_WORKSPACE,
                workspace_profiles: vec![WorkspaceProfileRef::new(
                    workspace.id.clone(),
                    DEFAULT_PROFILE_REVISION,
                )],
                software_install_commands: Vec::new(),
                environment_variables: Vec::new(),
            },
            draft_version: 1,
            created_at: SEEDED_AT.into(),
            updated_at: SEEDED_AT.into(),
            retired_at: None,
        };
        let sandbox_revision = sandbox
            .publish(DEFAULT_PROFILE_REVISION, SEEDED_AT.into(), bounds, scanner)
            .await
            .map_err(profile_invalid)?;
        Ok(DefaultProfileRecords {
            agent,
            agent_revision,
            workspace,
            workspace_revision,
            sandbox,
            sandbox_revision,
        })
    }
}

impl MongoProfileStore {
    pub(crate) async fn seed_default_profiles(
        &self,
        spec: &DefaultProfileSpec,
        bounds: ProfileCapacityBounds,
        scanner: &dyn layer2_core::profiles::PublicValueScanner,
    ) -> Result<(), StoreError> {
        let records = spec.records(bounds, scanner).await?;
        let documents = [
            (
                &self.agent_profiles,
                published_profile(
                    agent_profiles::document(&records.agent).map_err(profile_store_error)?,
                ),
            ),
            (
                &self.agent_revisions,
                revision_document(
                    records.agent.id.as_str(),
                    DEFAULT_PROFILE_REVISION,
                    &records.agent_revision,
                )
                .map_err(profile_store_error)?,
            ),
            (
                &self.workspace_profiles,
                published_profile(
                    workspace_profiles::document(&records.workspace)
                        .map_err(profile_store_error)?,
                ),
            ),
            (
                &self.workspace_revisions,
                revision_document(
                    records.workspace.id.as_str(),
                    DEFAULT_PROFILE_REVISION,
                    &records.workspace_revision,
                )
                .map_err(profile_store_error)?,
            ),
            (
                &self.sandbox_profiles,
                published_profile(
                    sandbox_profiles::document(&records.sandbox).map_err(profile_store_error)?,
                ),
            ),
            (
                &self.sandbox_revisions,
                revision_document(
                    records.sandbox.id.as_str(),
                    DEFAULT_PROFILE_REVISION,
                    &records.sandbox_revision,
                )
                .map_err(profile_store_error)?,
            ),
        ];
        for (collection, document) in documents {
            insert_if_absent(collection, &document).await?;
        }
        if self.verify_default_profiles(&records).await? {
            tracing::info!("deployment-owned default profiles are ready");
        }
        Ok(())
    }

    async fn verify_default_profiles(
        &self,
        expected: &DefaultProfileRecords,
    ) -> Result<bool, StoreError> {
        let mut healthy = verify_profile(
            &self.agent_profiles,
            expected.agent.id.as_str(),
            &expected.agent,
            agent_profiles::read_profile,
            |profile| &mut profile.uses_count,
        )
        .await?;
        healthy &= verify_revision(
            &self.agent_revisions,
            expected.agent.id.as_str(),
            &expected.agent_revision,
            agent_profiles::read_revision,
        )
        .await?;
        healthy &= verify_profile(
            &self.workspace_profiles,
            expected.workspace.id.as_str(),
            &expected.workspace,
            workspace_profiles::read_profile,
            |profile| &mut profile.uses_count,
        )
        .await?;
        healthy &= verify_revision(
            &self.workspace_revisions,
            expected.workspace.id.as_str(),
            &expected.workspace_revision,
            workspace_profiles::read_revision,
        )
        .await?;
        healthy &= verify_profile(
            &self.sandbox_profiles,
            expected.sandbox.id.as_str(),
            &expected.sandbox,
            sandbox_profiles::read_profile,
            |profile| &mut profile.uses_count,
        )
        .await?;
        healthy &= verify_revision(
            &self.sandbox_revisions,
            expected.sandbox.id.as_str(),
            &expected.sandbox_revision,
            sandbox_profiles::read_revision,
        )
        .await?;
        Ok(healthy)
    }
}

fn profile_invalid(error: layer2_core::profiles::ProfileError) -> StoreError {
    StoreError::Malformed(format!("default profile configuration is invalid: {error}"))
}

fn profile_store_error(error: ProfileStoreError) -> StoreError {
    match error {
        ProfileStoreError::Unavailable(message) => StoreError::Unavailable(message),
        error => StoreError::Malformed(error.to_string()),
    }
}

fn published_profile(mut document: Document) -> Document {
    document.insert("last_revision", i64::from(DEFAULT_PROFILE_REVISION));
    document.insert("latest_published_at", SEEDED_AT);
    document
}

async fn insert_if_absent(
    collection: &Collection<Document>,
    document: &Document,
) -> Result<(), StoreError> {
    let id = document
        .get("_id")
        .cloned()
        .ok_or_else(|| StoreError::Malformed("default profile document has no id".into()))?;
    collection
        .update_one(doc! { "_id": id }, doc! { "$setOnInsert": document })
        .upsert(true)
        .await
        .map(|_| ())
        .map_err(|error| {
            StoreError::Unavailable(format!(
                "seed {} default profile document: {error}",
                collection.name()
            ))
        })
}

async fn verify_profile<T: PartialEq + DeserializeOwned>(
    collection: &Collection<Document>,
    id: &str,
    expected: &T,
    decode: fn(&Document) -> Result<T, ProfileStoreError>,
    usage: fn(&mut T) -> &mut u64,
) -> Result<bool, StoreError> {
    let document = required_document(collection, id).await?;
    let publication_matches = document
        .get_i64("last_revision")
        .is_ok_and(|revision| revision == i64::from(DEFAULT_PROFILE_REVISION))
        && document.get("latest_published_at") == Some(&Bson::String(SEEDED_AT.into()));
    if !publication_matches {
        return Err(StoreError::Malformed(format!(
            "deployment default {id} disagrees with this process configuration"
        )));
    }
    let Some(actual) = decode_default(collection, id, &document, decode)? else {
        return Ok(false);
    };
    if default_profile_matches(actual, expected, usage) {
        return Ok(true);
    }
    Err(StoreError::Malformed(format!(
        "deployment default {id} disagrees with this process configuration"
    )))
}

fn default_profile_matches<T: PartialEq>(
    mut actual: T,
    expected: &T,
    usage: fn(&mut T) -> &mut u64,
) -> bool {
    // Usage is derived activity, not deployment configuration. Never write it back.
    *usage(&mut actual) = 0;
    &actual == expected
}

async fn verify_revision<T: PartialEq + DeserializeOwned>(
    collection: &Collection<Document>,
    profile_id: &str,
    expected: &T,
    decode: fn(&Document) -> Result<T, ProfileStoreError>,
) -> Result<bool, StoreError> {
    let id = format!("{profile_id}:{DEFAULT_PROFILE_REVISION:010}");
    let document = required_document(collection, &id).await?;
    if document.get_str("profile_id").ok() != Some(profile_id)
        || !super::sandbox_mongo::i64_field_matches(
            &document,
            "revision",
            i64::from(DEFAULT_PROFILE_REVISION),
        )
    {
        return Err(StoreError::Malformed(format!(
            "deployment default revision {id} disagrees with this process configuration"
        )));
    }
    let Some(actual) = decode_default(collection, &id, &document, decode)? else {
        return Ok(false);
    };
    if &actual == expected {
        return Ok(true);
    }
    Err(StoreError::Malformed(format!(
        "deployment default revision {id} disagrees with this process configuration"
    )))
}

fn decode_default<T: DeserializeOwned>(
    collection: &Collection<Document>,
    id: &str,
    document: &Document,
    decode: fn(&Document) -> Result<T, ProfileStoreError>,
) -> Result<Option<T>, StoreError> {
    require_version(document).map_err(StoreError::Malformed)?;
    match decode(document) {
        Ok(record) => Ok(Some(record)),
        Err(ProfileStoreError::Corrupt(_))
            if !document.contains_key("account_erasure_retained")
                && read_record::<T>(document).is_err() =>
        {
            tracing::error!(
                collection = collection.name(),
                document_id = id,
                failure = "malformed_profile_payload",
                "Deployment default Profile quarantined in place"
            );
            Ok(None)
        }
        Err(error) => Err(profile_store_error(error)),
    }
}

async fn required_document(
    collection: &Collection<Document>,
    id: &str,
) -> Result<Document, StoreError> {
    collection
        .find_one(doc! { "_id": id })
        .await
        .map_err(|error| StoreError::Unavailable(error.to_string()))?
        .ok_or_else(|| StoreError::Malformed(format!("default profile document {id} is missing")))
}

#[cfg(test)]
#[path = "default_profiles_tests.rs"]
mod tests;
