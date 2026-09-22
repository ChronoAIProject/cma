use std::collections::BTreeSet;

use super::*;
use crate::sandbox::agent::profile_agent_creation_key;
use crate::sandbox::SandboxStatus;
use crate::SandboxId;

impl AgentService {
    /// Reconcile pinned children without recreating stopped or deleted Agents.
    pub async fn reconcile_profile_agents(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<usize, AgentServiceError> {
        let Some(sandbox) = self
            .store
            .get_sandbox(sandbox_id)
            .await
            .map_err(store_error)?
        else {
            return Ok(0);
        };
        if sandbox.status != SandboxStatus::Active {
            return Ok(0);
        }
        self.runtime_capabilities
            .require([RuntimeCapability::Conversation])
            .map_err(|capability| AgentServiceError::CapabilityUnsupported { capability })?;
        let mut page = PageRequest::default();
        let mut seen = BTreeSet::new();
        let mut created = 0;
        loop {
            let workspaces = self
                .store
                .list_workspaces(sandbox_id, &page)
                .await
                .map_err(store_error)?;
            for workspace in workspaces.items {
                if workspace.sandbox_id != sandbox.id || workspace.owner != sandbox.owner {
                    return Err(AgentServiceError::Corrupt(
                        "automatic agent workspace lineage changed".into(),
                    ));
                }
                if workspace.status != WorkspaceStatus::Ready || workspace.resolved_commit.is_none()
                {
                    continue;
                }
                for (index, reference) in
                    workspace.profile.content.agent_profiles.iter().enumerate()
                {
                    let index = u16::try_from(index).map_err(|_| {
                        AgentServiceError::Corrupt("automatic agent index is invalid".into())
                    })?;
                    let revision = self.exact_revision(reference).await?;
                    let admission = AgentAdmission {
                        binding: None,
                        created_by: None,
                        owner: workspace.owner.clone(),
                        workspace_id: workspace.id.clone(),
                        agent_id: AgentId::generate(),
                        worktree_id: WorktreeId::generate(),
                        profile: AgentProfileSnapshot::from(&revision),
                        capabilities: self.runtime_capabilities.clone(),
                        title: None,
                        first_message: None,
                        origin: AgentOrigin::WorkspaceProfile { index },
                        creation_key: profile_agent_creation_key(&workspace.id, index),
                        event_at: now_rfc3339(),
                    };
                    match self
                        .store
                        .admit_agent(admission)
                        .await
                        .map_err(store_error)?
                    {
                        AgentAdmissionResult::Created(creation) => {
                            self.record_profile_use(&creation.agent).await;
                            created += 1;
                        }
                        AgentAdmissionResult::Existing(_) => {}
                    }
                }
            }
            let Some(next) = workspaces.next else {
                return Ok(created);
            };
            if page
                .cursor
                .as_ref()
                .is_some_and(|previous| previous >= &next)
                || !seen.insert(next.clone())
            {
                return Err(AgentServiceError::Corrupt(
                    "automatic agent workspace scan did not advance".into(),
                ));
            }
            page.cursor = Some(next);
        }
    }
}
