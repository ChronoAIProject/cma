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
    pub workspace: WorkspaceProfileRevision,
    pub agent: AgentProfileRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriggerReferenceError {
    #[error("trigger_reference_unavailable: {field}")]
    Unavailable { field: &'static str },
    #[error("trigger_reference_not_public: {field}")]
    NotPublic { field: &'static str },
}

impl TriggerReferenceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "trigger_reference_unavailable",
            Self::NotPublic { .. } => "trigger_reference_not_public",
        }
    }

    pub fn field(&self) -> &'static str {
        match self {
            Self::Unavailable { field } | Self::NotPublic { field } => field,
        }
    }
}

#[async_trait]
pub trait TriggerProfileResolver: Send + Sync {
    /// Authorizes every target against current profile visibility and retirement state.
    ///
    /// This is the publication-time path. Durable Trigger stores must evaluate it in the
    /// same consistent write boundary as inserting the immutable Trigger revision.
    async fn resolve_trigger_profiles(
        &self,
        trigger_owner: &SubjectId,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, TriggerReferenceError>;

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
        )?;

        let workspace_profile = find_workspace(self, target.workspace_profile.id.as_str()).await?;
        require_public(
            trigger_owner,
            &workspace_profile.owner,
            workspace_profile.visibility,
            workspace_profile.retired_at.as_deref(),
            workspace_field,
        )?;

        let agent_profile = find_agent(self, target.agent_profile.id.as_str()).await?;
        require_public(
            trigger_owner,
            &agent_profile.owner,
            agent_profile.visibility,
            agent_profile.retired_at.as_deref(),
            agent_field,
        )?;

        self.read_trigger_profiles(target).await
    }

    async fn read_trigger_profiles(
        &self,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, TriggerReferenceError> {
        let sandbox_field = "target.sandbox_profile";
        let workspace_field = "target.workspace_profile";
        let agent_field = "target.agent_profile";

        let sandbox = self
            .sandbox_profile_revision(&target.sandbox_profile.id, target.sandbox_profile.revision)
            .await
            .map_err(|_| unavailable(sandbox_field))?
            .ok_or_else(|| unavailable(sandbox_field))?;
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
            workspace,
            agent,
        })
    }
}

fn require_public(
    trigger_owner: &SubjectId,
    profile_owner: &SubjectId,
    visibility: ProfileVisibility,
    retired_at: Option<&str>,
    field: &'static str,
) -> Result<(), TriggerReferenceError> {
    if retired_at.is_some()
        || (visibility == ProfileVisibility::Private && trigger_owner != profile_owner)
    {
        return Err(unavailable(field));
    }
    if visibility == ProfileVisibility::Private {
        return Err(TriggerReferenceError::NotPublic { field });
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
