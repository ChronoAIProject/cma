use std::collections::BTreeSet;

use crate::profiles::{AgentProfileRef, ProfilePageRequest};
use crate::sandbox::store::PageRequest;
use crate::sandbox::{
    CreateSandbox, CreateWorkspace, Sandbox, SandboxOrigin, SandboxStatus, Workspace,
    WorkspaceServiceError, WorkspaceStatus,
};
use crate::{AgentProfileId, Caller, Capability, PlatformDefaults};

use super::{AgentContextError, AgentContextService};

impl AgentContextService {
    pub(super) async fn resolve_agent_profile(
        &self,
        caller: &Caller,
        id: Option<AgentProfileId>,
        defaults: &PlatformDefaults,
    ) -> Result<AgentProfileRef, AgentContextError> {
        let Some(id) = id else {
            return Ok(defaults.agent_profile.clone());
        };
        self.profiles
            .agent_profile(&id)
            .await
            .map_err(|_| AgentContextError::Unavailable)?
            .filter(|profile| {
                profile.id == id && profile.retired_at.is_none() && profile.owner == caller.subject
            })
            .ok_or(AgentContextError::InvalidOverride)?;
        let revision = self
            .profiles
            .list_agent_profile_revisions(
                &id,
                &ProfilePageRequest {
                    limit: 1,
                    cursor: None,
                },
            )
            .await
            .map_err(|_| AgentContextError::Unavailable)?
            .items
            .into_iter()
            .next()
            .ok_or(AgentContextError::InvalidOverride)?;
        if revision.profile_id != id || revision.revision == 0 {
            return Err(AgentContextError::Unavailable);
        }
        Ok(AgentProfileRef::new(id, revision.revision))
    }

    pub(super) async fn default_sandbox(
        &self,
        caller: &Caller,
        defaults: &PlatformDefaults,
    ) -> Result<Sandbox, AgentContextError> {
        let mut page = PageRequest::default();
        let mut cursors = BTreeSet::new();
        let mut selected: Option<Sandbox> = None;
        loop {
            let candidates = self.sandboxes.list_own(caller, &page).await?;
            for candidate in candidates.items {
                if candidate.owner != caller.subject {
                    return Err(AgentContextError::Unavailable);
                }
                if candidate.profile.id == defaults.sandbox_profile.id
                    && candidate.profile.revision == defaults.sandbox_profile.revision
                    && !matches!(
                        candidate.status,
                        SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired
                    )
                    && selected.as_ref().is_none_or(|previous| {
                        (&candidate.last_activity_at, &candidate.id)
                            > (&previous.last_activity_at, &previous.id)
                    })
                {
                    selected = Some(candidate);
                }
            }
            let Some(next) = candidates.next else {
                break;
            };
            if page.cursor.as_ref().is_some_and(|old| old >= &next) || !cursors.insert(next.clone())
            {
                return Err(AgentContextError::Unavailable);
            }
            page.cursor = Some(next);
        }
        if let Some(sandbox) = selected {
            return Ok(sandbox);
        }
        if !caller.authorization.allows(Capability::CreateSandbox) {
            return Err(AgentContextError::SandboxRequired);
        }
        Ok(self
            .sandboxes
            .create(
                caller,
                CreateSandbox {
                    profile: defaults.sandbox_profile.clone(),
                    name: None,
                    confirmed_digest: None,
                    origin: SandboxOrigin::Manual,
                },
            )
            .await?)
    }

    pub(super) async fn default_workspace(
        &self,
        caller: &Caller,
        sandbox: &Sandbox,
        defaults: &PlatformDefaults,
    ) -> Result<Workspace, AgentContextError> {
        if let Some(workspace) = self.find_workspace(caller, sandbox, defaults).await? {
            return Ok(workspace);
        }
        match self
            .workspaces
            .create(
                caller,
                &sandbox.id,
                CreateWorkspace {
                    profile: defaults.workspace_profile.clone(),
                    confirmed_digest: None,
                },
            )
            .await
        {
            Ok(workspace) => Ok(workspace),
            Err(WorkspaceServiceError::NameConflict) => {
                self.find_workspace(caller, sandbox, defaults).await?.ok_or(
                    AgentContextError::Workspace(WorkspaceServiceError::NameConflict),
                )
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn find_workspace(
        &self,
        caller: &Caller,
        sandbox: &Sandbox,
        defaults: &PlatformDefaults,
    ) -> Result<Option<Workspace>, AgentContextError> {
        let mut page = PageRequest::default();
        let mut cursors = BTreeSet::new();
        loop {
            let candidates = self.workspaces.list(caller, &sandbox.id, &page).await?;
            for candidate in candidates.items {
                if candidate.owner != caller.subject || candidate.sandbox_id != sandbox.id {
                    return Err(AgentContextError::Unavailable);
                }
                if candidate.profile.id == defaults.workspace_profile.id
                    && candidate.profile.revision == defaults.workspace_profile.revision
                    && matches!(
                        candidate.status,
                        WorkspaceStatus::Ready | WorkspaceStatus::Preparing
                    )
                {
                    return Ok(Some(candidate));
                }
            }
            let Some(next) = candidates.next else {
                return Ok(None);
            };
            if page.cursor.as_ref().is_some_and(|old| old >= &next) || !cursors.insert(next.clone())
            {
                return Err(AgentContextError::Unavailable);
            }
            page.cursor = Some(next);
        }
    }
}
