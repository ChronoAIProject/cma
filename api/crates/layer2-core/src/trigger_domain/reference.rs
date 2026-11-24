use async_trait::async_trait;

use crate::ids::SubjectId;
use crate::profiles::{
    AgentProfile, AgentProfileRevision, AgentProfileStore, InMemoryProfileStore,
    ProfilePageRequest, ProfileVisibility, SandboxProfile, SandboxProfileRevision,
    SandboxProfileStore, WorkspaceProfile, WorkspaceProfileRevision, WorkspaceProfileStore,
};

use super::TriggerTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTriggerProfiles {
    pub sandbox: SandboxProfileRevision,
    pub sandbox_workspaces: Vec<WorkspaceProfileRevision>,
    pub workspace: WorkspaceProfileRevision,
    pub agent: AgentProfileRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriggerReferenceError {
    #[error("trigger_reference_unavailable: {field}")]
    Unavailable { field: &'static str },
    #[error("trigger_reference_not_public: {field}")]
    NotPublic { field: &'static str },
    #[error("trigger_image_access_denied: {field}")]
    ImageAccessDenied { field: &'static str },
    #[error("trigger_image_sign_in_required: {field}")]
    ImageSignInRequired { field: &'static str },
    #[error("trigger_image_selection_unsupported: {field}")]
    ImageSelectionUnsupported { field: &'static str },
}

impl TriggerReferenceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "trigger_reference_unavailable",
            Self::NotPublic { .. } => "trigger_reference_not_public",
            Self::ImageAccessDenied { .. } => "trigger_image_access_denied",
            Self::ImageSignInRequired { .. } => "trigger_image_sign_in_required",
            Self::ImageSelectionUnsupported { .. } => "trigger_image_selection_unsupported",
        }
    }

    pub fn field(&self) -> &'static str {
        match self {
            Self::Unavailable { field }
            | Self::NotPublic { field }
            | Self::ImageAccessDenied { field }
            | Self::ImageSignInRequired { field }
            | Self::ImageSelectionUnsupported { field } => field,
        }
    }
}

#[async_trait]
pub trait TriggerProfileReader: Send + Sync {
    /// Reads exact immutable revisions that were authorized when the Trigger was published.
    ///
    /// Current profile visibility and retirement state must not invalidate an existing
    /// published Trigger revision.
    async fn read_trigger_profiles(
        &self,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, TriggerReferenceError>;
}

#[async_trait]
pub trait TriggerProfileResolver: TriggerProfileReader {
    /// Authorizes every target against current profile visibility and retirement state.
    ///
    /// This is the publication-time path. Durable Trigger stores must evaluate it in the
    /// same consistent write boundary as inserting the immutable Trigger revision.
    async fn resolve_trigger_profiles(
        &self,
        trigger_owner: &SubjectId,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, TriggerReferenceError>;
}

/// Converts an Agent Manager inline Trigger target into the retained immutable
/// profile references used by the launch workers.  The generated profiles are
/// an internal persistence detail; callers continue to author only the inline
/// target.
#[async_trait]
pub trait TriggerInlineTargetMaterializer: Send + Sync {
    async fn materialize(
        &self,
        owner: &SubjectId,
        target: &TriggerTarget,
    ) -> Result<TriggerTarget, TriggerReferenceError>;

    /// Materialize an inline target with the caller's delegated service
    /// credential when an external resolver needs to verify Image Manager
    /// authority. Compatibility implementations may use the legacy method.
    async fn materialize_with_credential(
        &self,
        owner: &SubjectId,
        target: &TriggerTarget,
        _delegated_credential: Option<&str>,
    ) -> Result<TriggerTarget, TriggerReferenceError> {
        self.materialize(owner, target).await
    }

    /// The existing target comes from the publisher's stored draft, so a
    /// materializer can preserve an immutable selection during other edits.
    async fn materialize_draft_update(
        &self,
        owner: &SubjectId,
        target: &TriggerTarget,
        _existing: &TriggerTarget,
        delegated_credential: Option<&str>,
    ) -> Result<TriggerTarget, TriggerReferenceError> {
        self.materialize_with_credential(owner, target, delegated_credential)
            .await
    }
}

#[async_trait]
impl TriggerProfileReader for InMemoryProfileStore {
    async fn read_trigger_profiles(
        &self,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, TriggerReferenceError> {
        let sandbox_field = "target.sandbox_profile";
        let sandbox_workspaces_field = "target.sandbox_profile.workspace_profiles";
        let workspace_field = "target.workspace_profile";
        let agent_field = "target.agent_profile";

        let sandbox = self
            .sandbox_profile_revision(&target.sandbox_profile.id, target.sandbox_profile.revision)
            .await
            .map_err(|_| unavailable(sandbox_field))?
            .ok_or_else(|| unavailable(sandbox_field))?;
        let mut sandbox_workspaces = Vec::with_capacity(sandbox.content.workspace_profiles.len());
        for reference in &sandbox.content.workspace_profiles {
            let revision = self
                .workspace_profile_revision(&reference.id, reference.revision)
                .await
                .map_err(|_| unavailable(sandbox_workspaces_field))?
                .ok_or_else(|| unavailable(sandbox_workspaces_field))?;
            sandbox_workspaces.push(revision);
        }
        let workspace = self
            .workspace_profile_revision(
                &target.workspace_profile.id,
                target.workspace_profile.revision,
            )
            .await
            .map_err(|_| unavailable(workspace_field))?
            .ok_or_else(|| unavailable(workspace_field))?;
        let agent = self
            .agent_profile_revision(&target.agent_profile.id, target.agent_profile.revision)
            .await
            .map_err(|_| unavailable(agent_field))?
            .ok_or_else(|| unavailable(agent_field))?;

        Ok(ResolvedTriggerProfiles {
            sandbox,
            sandbox_workspaces,
            workspace,
            agent,
        })
    }
}

#[async_trait]
impl TriggerProfileResolver for InMemoryProfileStore {
    async fn resolve_trigger_profiles(
        &self,
        trigger_owner: &SubjectId,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, TriggerReferenceError> {
        let sandbox_field = "target.sandbox_profile";
        let workspace_field = "target.workspace_profile";
        let agent_field = "target.agent_profile";

        let sandbox_profile = find_sandbox(self, target.sandbox_profile.id.as_str()).await?;
        require_public(
            trigger_owner,
            &sandbox_profile.owner,
            sandbox_profile.visibility,
            sandbox_profile.retired_at.as_deref(),
            sandbox_field,
            target.inline.is_some(),
        )?;

        let workspace_profile = find_workspace(self, target.workspace_profile.id.as_str()).await?;
        require_public(
            trigger_owner,
            &workspace_profile.owner,
            workspace_profile.visibility,
            workspace_profile.retired_at.as_deref(),
            workspace_field,
            target.inline.is_some(),
        )?;

        let agent_profile = find_agent(self, target.agent_profile.id.as_str()).await?;
        require_public(
            trigger_owner,
            &agent_profile.owner,
            agent_profile.visibility,
            agent_profile.retired_at.as_deref(),
            agent_field,
            target.inline.is_some(),
        )?;

        self.read_trigger_profiles(target).await
    }
}

fn require_public(
    trigger_owner: &SubjectId,
    profile_owner: &SubjectId,
    visibility: ProfileVisibility,
    retired_at: Option<&str>,
    field: &'static str,
    allow_owned_inline: bool,
) -> Result<(), TriggerReferenceError> {
    if retired_at.is_some() {
        return Err(unavailable(field));
    }
    if visibility == ProfileVisibility::Private {
        if !allow_owned_inline {
            return Err(TriggerReferenceError::NotPublic { field });
        }
        if trigger_owner != profile_owner {
            // An inline target may use private generated snapshots only when the
            // snapshot belongs to the publisher. Keep foreign private rows opaque.
            return Err(unavailable(field));
        }
    }
    Ok(())
}

async fn find_sandbox(
    store: &InMemoryProfileStore,
    id: &str,
) -> Result<SandboxProfile, TriggerReferenceError> {
    let field = "target.sandbox_profile";
    let mut page = ProfilePageRequest {
        limit: 200,
        cursor: None,
    };
    loop {
        let result = store
            .list_sandbox_profiles(None, false, &page)
            .await
            .map_err(|_| unavailable(field))?;
        if let Some(profile) = result
            .items
            .into_iter()
            .find(|profile| profile.id.as_str() == id)
        {
            return Ok(profile);
        }
        match result.next {
            Some(next) => page.cursor = Some(next),
            None => return Err(unavailable(field)),
        }
    }
}

async fn find_workspace(
    store: &InMemoryProfileStore,
    id: &str,
) -> Result<WorkspaceProfile, TriggerReferenceError> {
    let field = "target.workspace_profile";
    let mut page = ProfilePageRequest {
        limit: 200,
        cursor: None,
    };
    loop {
        let result = store
            .list_workspace_profiles(None, false, &page)
            .await
            .map_err(|_| unavailable(field))?;
        if let Some(profile) = result
            .items
            .into_iter()
            .find(|profile| profile.id.as_str() == id)
        {
            return Ok(profile);
        }
        match result.next {
            Some(next) => page.cursor = Some(next),
            None => return Err(unavailable(field)),
        }
    }
}

async fn find_agent(
    store: &InMemoryProfileStore,
    id: &str,
) -> Result<AgentProfile, TriggerReferenceError> {
    let field = "target.agent_profile";
    let mut page = ProfilePageRequest {
        limit: 200,
        cursor: None,
    };
    loop {
        let result = store
            .list_agent_profiles(None, false, &page)
            .await
            .map_err(|_| unavailable(field))?;
        if let Some(profile) = result
            .items
            .into_iter()
            .find(|profile| profile.id.as_str() == id)
        {
            return Ok(profile);
        }
        match result.next {
            Some(next) => page.cursor = Some(next),
            None => return Err(unavailable(field)),
        }
    }
}

fn unavailable(field: &'static str) -> TriggerReferenceError {
    TriggerReferenceError::Unavailable { field }
}
