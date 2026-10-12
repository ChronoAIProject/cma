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

use super::profile_documents::revision_document;
use super::sandbox_mongo::MongoProfileStore;
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
                permission_level: PermissionLevel::Guarded,
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
            availability: Default::default(),
            id: SandboxProfileId(DEFAULT_SANDBOX_PROFILE_ID.into()),
            owner,
            name: "Default sandbox".into(),
            description: "Deployment-owned default sandbox profile.".into(),
            visibility: ProfileVisibility::Public,
            forked_from: None,
            uses_count: 0,
            draft: SandboxProfileContent {
                cpu_millis: self.cpu_millis,
                memory_mib: self.memory_mib,
                nyxid_services: Vec::new(),
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
        self.verify_default_profiles(&records).await?;
        tracing::info!("deployment-owned default profiles are ready");
        Ok(())
    }

    async fn verify_default_profiles(
        &self,
        expected: &DefaultProfileRecords,
    ) -> Result<(), StoreError> {
        verify_profile(
            &self.agent_profiles,
            expected.agent.id.as_str(),
            &expected.agent,
            agent_profiles::read_profile,
            |profile| &mut profile.uses_count,
        )
        .await?;
        verify_revision(
            &self.agent_revisions,
            expected.agent.id.as_str(),
            &expected.agent_revision,
            agent_profiles::read_revision,
        )
        .await?;
        verify_profile(
            &self.workspace_profiles,
            expected.workspace.id.as_str(),
            &expected.workspace,
            workspace_profiles::read_profile,
            |profile| &mut profile.uses_count,
        )
        .await?;
        verify_revision(
            &self.workspace_revisions,
            expected.workspace.id.as_str(),
            &expected.workspace_revision,
            workspace_profiles::read_revision,
        )
        .await?;
        verify_profile(
            &self.sandbox_profiles,
            expected.sandbox.id.as_str(),
            &expected.sandbox,
            sandbox_profiles::read_profile,
            |profile| &mut profile.uses_count,
        )
        .await?;
        verify_revision(
            &self.sandbox_revisions,
            expected.sandbox.id.as_str(),
            &expected.sandbox_revision,
            sandbox_profiles::read_revision,
        )
        .await
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

async fn verify_profile<T: PartialEq>(
    collection: &Collection<Document>,
    id: &str,
    expected: &T,
    decode: fn(&Document) -> Result<T, ProfileStoreError>,
    usage: fn(&mut T) -> &mut u64,
) -> Result<(), StoreError> {
    let document = required_document(collection, id).await?;
    let actual = decode(&document).map_err(profile_store_error)?;
    let publication_matches = document
        .get_i64("last_revision")
        .is_ok_and(|revision| revision == i64::from(DEFAULT_PROFILE_REVISION))
        && document.get("latest_published_at") == Some(&Bson::String(SEEDED_AT.into()));
    if default_profile_matches(actual, expected, usage) && publication_matches {
        Ok(())
    } else {
        Err(StoreError::Malformed(format!(
            "deployment default {id} disagrees with this process configuration"
        )))
    }
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

async fn verify_revision<T: PartialEq>(
    collection: &Collection<Document>,
    profile_id: &str,
    expected: &T,
    decode: fn(&Document) -> Result<T, ProfileStoreError>,
) -> Result<(), StoreError> {
    let id = format!("{profile_id}:{DEFAULT_PROFILE_REVISION:010}");
    let actual = decode(&required_document(collection, &id).await?).map_err(profile_store_error)?;
    if &actual == expected {
        Ok(())
    } else {
        Err(StoreError::Malformed(format!(
            "deployment default revision {id} disagrees with this process configuration"
        )))
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
mod tests {
    use super::*;

    fn bounds() -> ProfileCapacityBounds {
        ProfileCapacityBounds {
            min_cpu_millis: 100,
            max_cpu_millis: 8_000,
            min_memory_mib: 128,
            max_memory_mib: 16_384,
        }
    }

    #[tokio::test]
    async fn records_are_public_deterministic_and_reference_each_other() {
        let spec = DefaultProfileSpec {
            cpu_millis: 1_000,
            memory_mib: 1_024,
            agent_model: "model-a".into(),
            agent_effort: ThinkingEffort::High,
        };
        let first = spec.records(bounds(), &[]).await.unwrap();
        let second = spec.records(bounds(), &[]).await.unwrap();
        assert_eq!(first.agent, second.agent);
        assert_eq!(first.agent_revision, second.agent_revision);
        assert_eq!(first.workspace_revision, second.workspace_revision);
        assert_eq!(first.sandbox_revision, second.sandbox_revision);
        assert_eq!(first.agent.owner.as_str(), SYSTEM_OWNER);
        assert_eq!(first.agent.visibility, ProfileVisibility::Public);
        assert_eq!(
            first.workspace.draft.agent_profiles,
            vec![AgentProfileRef::new(
                first.agent.id,
                DEFAULT_PROFILE_REVISION
            )]
        );
        assert_eq!(
            first.sandbox.draft.workspace_profiles,
            vec![WorkspaceProfileRef::new(
                first.workspace.id,
                DEFAULT_PROFILE_REVISION
            )]
        );
    }

    #[tokio::test]
    async fn used_default_profiles_remain_valid_without_accepting_configuration_drift() {
        let spec = DefaultProfileSpec {
            cpu_millis: 1_000,
            memory_mib: 1_024,
            agent_model: "model-a".into(),
            agent_effort: ThinkingEffort::High,
        };
        let expected = spec.records(bounds(), &[]).await.unwrap();
        for count in [1, 10, i64::MAX as u64] {
            let mut agent = expected.agent.clone();
            agent.uses_count = count;
            assert!(default_profile_matches(
                agent.clone(),
                &expected.agent,
                |p| &mut p.uses_count
            ));
            assert_eq!(agent.uses_count, count);
            agent.draft.model = Some("different-model".into());
            assert!(!default_profile_matches(agent, &expected.agent, |p| &mut p.uses_count));

            let mut workspace = expected.workspace.clone();
            workspace.uses_count = count;
            assert!(default_profile_matches(
                workspace.clone(),
                &expected.workspace,
                |p| &mut p.uses_count
            ));
            workspace.draft_version += 1;
            assert!(!default_profile_matches(
                workspace,
                &expected.workspace,
                |p| &mut p.uses_count
            ));

            let mut sandbox = expected.sandbox.clone();
            sandbox.uses_count = count;
            assert!(default_profile_matches(
                sandbox.clone(),
                &expected.sandbox,
                |p| &mut p.uses_count
            ));
            sandbox.draft.cpu_millis += 100;
            assert!(!default_profile_matches(sandbox, &expected.sandbox, |p| {
                &mut p.uses_count
            }));
        }
    }

    #[tokio::test]
    async fn deployment_reservation_must_fit_publication_bounds() {
        let spec = DefaultProfileSpec {
            cpu_millis: 99,
            memory_mib: 1_024,
            agent_model: "model-a".into(),
            agent_effort: ThinkingEffort::High,
        };
        assert!(spec.records(bounds(), &[]).await.is_err());
    }
}
