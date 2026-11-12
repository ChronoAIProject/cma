use std::sync::Arc;

use crate::agent_lifecycle::ImageResolver;
use async_trait::async_trait;
use layer1_public::rest::AgentLifecycleImageReference;
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
    capacity: layer3_adapter::SandboxCapacity,
    image_resolver: Arc<dyn ImageResolver>,
}

impl InlineTriggerProfileMaterializer {
    pub(crate) fn new(
        profile_store: Arc<dyn layer2_core::bots::BotProfileStore>,
        bounds: ProfileCapacityBounds,
        capacity: layer3_adapter::SandboxCapacity,
        scanner: Arc<dyn layer2_core::profiles::PublicValueScanner>,
        image_resolver: Arc<dyn ImageResolver>,
    ) -> Arc<Self> {
        Arc::new(Self {
            profiles: PrivateProfileCreationService::new(profile_store, bounds, scanner),
            capacity,
            image_resolver,
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
        self.materialize_with_credential(owner, target, None).await
    }

    async fn materialize_with_credential(
        &self,
        owner: &SubjectId,
        target: &TriggerTarget,
        delegated_credential: Option<&str>,
    ) -> Result<TriggerTarget, TriggerReferenceError> {
        let Some(inline) = &target.inline else {
            return Ok(target.clone());
        };
        let mut inline = inline.clone();
        if let Some(image) = inline.image.as_mut() {
            image.reference = Some(
                self.image_resolver
                    .resolve(
                        delegated_credential,
                        &AgentLifecycleImageReference {
                            id: image.id.clone(),
                            version: image.version.clone(),
                        },
                    )
                    .await
                    .map_err(image_reference_error)?,
            );
        }

        inline
            .validate()
            .map_err(|error| unavailable(error.field))?;
        let key_digest =
            content_digest(&(&inline, self.capacity)).map_err(|_| unavailable("target.inline"))?;
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
                    cpu_millis: self.capacity.cpu_millis,
                    memory_mib: self.capacity.memory_mib,
                    runtime_image: inline
                        .image
                        .as_ref()
                        .and_then(|image| image.reference.clone()),
                    nyxid_services: inline.agent.services.clone(),
                    agent_provider: AgentProvider::Codex,
                    model_access: ModelAccess::ChronoaiManaged,
                    max_workspaces: 16,
                    max_agents_per_workspace: 16,
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
            inline: Some(inline),
        })
    }
}

fn unavailable(field: &'static str) -> TriggerReferenceError {
    TriggerReferenceError::Unavailable { field }
}

fn image_reference_error(
    error: layer1_public::rest::AgentLifecycleProvisioningError,
) -> TriggerReferenceError {
    const FIELD: &str = "target.inline.image";
    match error {
        layer1_public::rest::AgentLifecycleProvisioningError::ImageSelectionUnsupported => {
            TriggerReferenceError::ImageSelectionUnsupported { field: FIELD }
        }
        _ => unavailable(FIELD),
    }
}

#[cfg(test)]
#[path = "inline_tests.rs"]
mod tests;
