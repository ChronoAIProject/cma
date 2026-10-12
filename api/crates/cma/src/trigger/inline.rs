use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::profiles::{
    content_digest, AgentProfileRef, AgentProvider, ModelAccess, PrivateProfileCreationService,
    ProfileCapacityBounds, SandboxProfileContent, SandboxProfileRef, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileRef,
};
use layer2_core::trigger_domain::{
    TriggerInlineTargetMaterializer, TriggerReferenceError, TriggerTarget,
};
use layer2_core::SubjectId;

/// Materializes the inline Trigger authoring shape into private immutable
/// profile revisions used by the existing launch worker. The profile rows are
/// deliberately private and deterministic; the inline target remains on the
/// Trigger so the user never has to manage those rows.
pub(crate) struct InlineTriggerProfileMaterializer {
    profiles: Arc<PrivateProfileCreationService>,
    bounds: ProfileCapacityBounds,
}

impl InlineTriggerProfileMaterializer {
    pub(crate) fn new(
        profile_store: Arc<dyn layer2_core::bots::BotProfileStore>,
        bounds: ProfileCapacityBounds,
        scanner: Arc<dyn layer2_core::profiles::PublicValueScanner>,
    ) -> Arc<Self> {
        Arc::new(Self {
            profiles: PrivateProfileCreationService::new(profile_store, bounds, scanner),
            bounds,
        })
    }
}

#[async_trait]
impl TriggerInlineTargetMaterializer for InlineTriggerProfileMaterializer {
    async fn materialize(
        &self,
        owner: &SubjectId,
        target: &TriggerTarget,
    ) -> Result<TriggerTarget, TriggerReferenceError> {
        let Some(inline) = &target.inline else {
            return Ok(target.clone());
        };
        inline
            .validate()
            .map_err(|error| unavailable(error.field))?;
        let key_digest = content_digest(inline).map_err(|_| unavailable("target.inline"))?;
        let key = format!("trigger-inline:{key_digest}");
        let agent = self
            .profiles
            .create_agent_for_owner(
                owner,
                &format!("{key}:agent"),
                "Inline Trigger Agent".into(),
                "Internal Agent Manager Trigger configuration".into(),
                layer2_core::profiles::AgentProfileContent {
                    isolated_agent: false,
                    model: inline.agent.model.clone(),
                    thinking_effort: inline.agent.reasoning_effort,
                    permission_level: inline.agent.permission_mode,
                    skills: inline.agent.skills.clone(),
                    system_prompt: inline.agent.system_prompt.clone(),
                },
            )
            .await
            .map_err(|_| unavailable("target.inline.agent"))?;
        let agent_ref = AgentProfileRef::new(agent.profile_id.clone(), agent.revision);

        let location = inline
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.repository.as_ref())
            .map(|repository| WorkspaceLocation::GitRepository {
                url: repository.url.clone(),
                reference: repository.reference.clone(),
            })
            .unwrap_or_else(|| WorkspaceLocation::Directory {
                name: "workspace".into(),
            });
        let workspace = self
            .profiles
            .create_workspace_for_owner(
                owner,
                &format!("{key}:workspace"),
                "Inline Trigger Workspace".into(),
                "Internal Agent Manager Trigger workspace configuration".into(),
                WorkspaceProfileContent {
                    location,
                    name: "workspace".into(),
                    setup_commands: Vec::new(),
                    agent_profiles: vec![agent_ref],
                },
            )
            .await
            .map_err(|_| unavailable("target.inline.workspace"))?;
        let workspace_ref =
            WorkspaceProfileRef::new(workspace.profile_id.clone(), workspace.revision);
        let sandbox = self
            .profiles
            .create_sandbox_for_owner(
                owner,
                &format!("{key}:sandbox"),
                "Inline Trigger Sandbox".into(),
                "Internal Agent Manager Trigger sandbox configuration".into(),
                SandboxProfileContent {
                    cpu_millis: self.bounds.min_cpu_millis,
                    memory_mib: self.bounds.min_memory_mib,
                    runtime_image: inline
                        .image
                        .as_ref()
                        .and_then(|image| image.reference.clone()),
                    nyxid_services: inline.agent.services.clone(),
                    agent_provider: AgentProvider::Codex,
                    model_access: ModelAccess::ChronoaiManaged,
                    max_workspaces: 1,
                    max_agents_per_workspace: 1,
                    workspace_profiles: vec![workspace_ref.clone()],
                    software_install_commands: Vec::new(),
                    environment_variables: Vec::new(),
                },
            )
            .await
            .map_err(|_| unavailable("target.inline.image"))?;
        Ok(TriggerTarget {
            sandbox_profile: SandboxProfileRef::new(sandbox.profile_id, sandbox.revision),
            workspace_profile: workspace_ref,
            agent_profile: AgentProfileRef::new(agent.profile_id, agent.revision),
            inline: Some(inline.clone()),
        })
    }
}

fn unavailable(field: &'static str) -> TriggerReferenceError {
    TriggerReferenceError::Unavailable { field }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{
        profiles::{
            AgentProfileStore, InMemoryProfileStore, ProfileCapacityBounds, SandboxProfileStore,
            WorkspaceProfileStore,
        },
        trigger_domain::TriggerInlineTargetMaterializer,
    };

    fn bounds() -> ProfileCapacityBounds {
        ProfileCapacityBounds {
            min_cpu_millis: 100,
            max_cpu_millis: 8_000,
            min_memory_mib: 128,
            max_memory_mib: 16_384,
        }
    }

    fn target() -> TriggerTarget {
        serde_json::from_value(serde_json::json!({
            "inline": {
                "image": {
                    "id": "private-codex",
                    "version": "v3",
                    "reference": "registry.example/codex@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                "agent": {
                    "model": "gpt-5-codex",
                    "reasoning_effort": "high",
                    "permission_mode": "guarded",
                    "skills": [{"source": "ornn", "id": "repo-search"}],
                    "system_prompt": "Keep changes small.",
                    "services": ["api-github"]
                },
                "workspace": {
                    "repository": {
                        "url": "https://github.com/chronoai/project",
                        "reference": "main"
                    }
                }
            }
        }))
        .expect("inline target should decode")
    }

    #[tokio::test]
    async fn materialization_is_deterministic_and_links_private_revisions() {
        let store = Arc::new(InMemoryProfileStore::default());
        let materializer = InlineTriggerProfileMaterializer::new(
            store.clone(),
            bounds(),
            Arc::new([] as [String; 0]),
        );
        let owner = SubjectId::new("trigger-owner");
        let source = target();

        let first = materializer.materialize(&owner, &source).await.unwrap();
        let second = materializer.materialize(&owner, &source).await.unwrap();
        assert_eq!(
            first, second,
            "same inline content must reuse its snapshots"
        );

        let agent = store
            .agent_profile(&first.agent_profile.id)
            .await
            .unwrap()
            .expect("generated agent profile");
        assert_eq!(agent.owner, owner);
        assert_eq!(
            agent.visibility,
            layer2_core::profiles::ProfileVisibility::Private
        );

        let agent_revision = store
            .agent_profile_revision(&first.agent_profile.id, first.agent_profile.revision)
            .await
            .unwrap()
            .expect("generated agent revision");
        let workspace_revision = store
            .workspace_profile_revision(
                &first.workspace_profile.id,
                first.workspace_profile.revision,
            )
            .await
            .unwrap()
            .expect("generated workspace revision");
        let sandbox_revision = store
            .sandbox_profile_revision(&first.sandbox_profile.id, first.sandbox_profile.revision)
            .await
            .unwrap()
            .expect("generated sandbox revision");

        assert_eq!(
            workspace_revision.content.agent_profiles,
            vec![AgentProfileRef::new(
                first.agent_profile.id.clone(),
                first.agent_profile.revision
            ),]
        );
        assert_eq!(
            sandbox_revision.content.workspace_profiles,
            vec![WorkspaceProfileRef::new(
                first.workspace_profile.id.clone(),
                first.workspace_profile.revision
            ),]
        );
        assert_eq!(
            sandbox_revision.content.runtime_image.as_deref(),
            Some("registry.example/codex@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(agent_revision.content.model.as_deref(), Some("gpt-5-codex"));
    }

    #[tokio::test]
    async fn changed_inline_content_and_owner_get_distinct_snapshots() {
        let store = Arc::new(InMemoryProfileStore::default());
        let materializer = InlineTriggerProfileMaterializer::new(
            store.clone(),
            bounds(),
            Arc::new([] as [String; 0]),
        );
        let owner = SubjectId::new("trigger-owner");
        let mut changed = target();
        changed.inline.as_mut().expect("inline target").agent.model =
            Some("gpt-5-codex-next".into());

        let first = materializer.materialize(&owner, &target()).await.unwrap();
        let second = materializer.materialize(&owner, &changed).await.unwrap();
        assert_ne!(first.agent_profile.id, second.agent_profile.id);
        assert_ne!(first.workspace_profile.id, second.workspace_profile.id);
        assert_ne!(first.sandbox_profile.id, second.sandbox_profile.id);

        let other_owner = SubjectId::new("other-owner");
        let other = materializer
            .materialize(&other_owner, &target())
            .await
            .unwrap();
        assert_ne!(first.agent_profile.id, other.agent_profile.id);
        let other_agent = store
            .agent_profile(&other.agent_profile.id)
            .await
            .unwrap()
            .expect("other owner's generated agent");
        assert_eq!(other_agent.owner, other_owner);
    }

    #[tokio::test]
    async fn invalid_inline_payload_is_rejected_before_profile_creation() {
        let store = Arc::new(InMemoryProfileStore::default());
        let materializer = InlineTriggerProfileMaterializer::new(
            store.clone(),
            bounds(),
            Arc::new([] as [String; 0]),
        );
        let mut invalid = target();
        invalid
            .inline
            .as_mut()
            .expect("inline target")
            .image
            .as_mut()
            .expect("image")
            .reference = None;

        let error = materializer
            .materialize(&SubjectId::new("trigger-owner"), &invalid)
            .await
            .expect_err("missing immutable image reference should fail");
        assert_eq!(error.field(), "target.inline.image.reference");
        assert!(store
            .list_agent_profiles(None, false, &Default::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }
}
