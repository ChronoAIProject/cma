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
    TriggerInlineTarget, TriggerInlineTargetMaterializer, TriggerReferenceError, TriggerTarget,
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
        self.materialize_target(owner, target, None, delegated_credential)
            .await
    }

    async fn materialize_draft_update(
        &self,
        owner: &SubjectId,
        target: &TriggerTarget,
        existing: &TriggerTarget,
        delegated_credential: Option<&str>,
    ) -> Result<TriggerTarget, TriggerReferenceError> {
        if self.image_resolver.retirement_enabled()
            && retains_image_settings(target.inline.as_ref(), existing.inline.as_ref())
        {
            // The edit changes no inline settings. Retain the exact profile
            // references from the stored target and avoid creating new profiles.
            return Ok(existing.clone());
        }
        self.materialize_target(
            owner,
            target,
            existing.inline.as_ref(),
            delegated_credential,
        )
        .await
    }
}

impl InlineTriggerProfileMaterializer {
    async fn materialize_target(
        &self,
        owner: &SubjectId,
        target: &TriggerTarget,
        existing: Option<&TriggerInlineTarget>,
        delegated_credential: Option<&str>,
    ) -> Result<TriggerTarget, TriggerReferenceError> {
        let Some(inline) = &target.inline else {
            return Ok(target.clone());
        };
        let mut inline = inline.clone();
        if let Some(image) = inline.image.as_mut() {
            let selection = AgentLifecycleImageReference {
                id: image.id.clone(),
                version: image.version.clone(),
            };
            // Only a stored, owner-authorized draft can supply the retained
            // digest. The request itself has no authority to attach a digest.
            let pinned = existing
                .and_then(|target| target.image.as_ref())
                .filter(|saved| image.id == saved.id && image.version == saved.version)
                .and_then(|saved| saved.reference.as_deref());
            let reference = match pinned {
                Some(reference) => {
                    self.image_resolver
                        .resolve_retained(delegated_credential, &selection, reference)
                        .await
                }
                None => {
                    self.image_resolver
                        .resolve(delegated_credential, &selection)
                        .await
                }
            };
            image.reference = Some(reference.map_err(image_reference_error)?);
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

fn retains_image_settings(
    requested: Option<&TriggerInlineTarget>,
    saved: Option<&TriggerInlineTarget>,
) -> bool {
    let (Some(requested), Some(saved)) = (requested, saved) else {
        return false;
    };
    let (Some(image), Some(pin)) = (&requested.image, &saved.image) else {
        return false;
    };
    // Clients can omit the server-resolved digest while resending identical
    // settings. An explicit digest must agree with the stored one.
    pin.reference.is_some()
        && image.id == pin.id
        && image.version == pin.version
        && image
            .reference
            .as_ref()
            .is_none_or(|reference| pin.reference.as_ref() == Some(reference))
        && requested.agent == saved.agent
        && requested.workspace == saved.workspace
}

fn unavailable(field: &'static str) -> TriggerReferenceError {
    TriggerReferenceError::Unavailable { field }
}

fn image_reference_error(
    error: layer1_public::rest::AgentLifecycleProvisioningError,
) -> TriggerReferenceError {
    const FIELD: &str = "target.inline.image";
    match error {
        layer1_public::rest::AgentLifecycleProvisioningError::Forbidden
        | layer1_public::rest::AgentLifecycleProvisioningError::ImageAccessDenied => {
            TriggerReferenceError::ImageAccessDenied { field: FIELD }
        }
        layer1_public::rest::AgentLifecycleProvisioningError::SignInRequired => {
            TriggerReferenceError::ImageSignInRequired { field: FIELD }
        }
        layer1_public::rest::AgentLifecycleProvisioningError::ImageSelectionUnsupported => {
            TriggerReferenceError::ImageSelectionUnsupported { field: FIELD }
        }
        _ => unavailable(FIELD),
    }
}

#[cfg(test)]
#[path = "inline_tests.rs"]
mod tests;
