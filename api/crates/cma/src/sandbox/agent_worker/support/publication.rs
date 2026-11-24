//! Rebuild only the durable mutation after a store-proven transaction abort.
use std::future::Future;
use std::time::{Duration, SystemTime};

use layer2_core::agent_ownership::execution_authority;
use layer2_core::sandbox::store::{
    AgentWorkMutation, AgentWorkPublication, AgentWorkPublicationLineage,
    AgentWorkPublicationParents, SandboxStoreError,
};
use layer2_core::sandbox::{
    Agent, Sandbox, SandboxStatus, Workspace, WorkspaceStatus, Worktree, WorktreeStatus,
};
use layer3_adapter::owner_protocol::TransportAuthority;

use super::{validate_lineage, AgentWorker};

const ATTEMPTS: usize = 5;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::sandbox::agent_worker) struct PublicationParents {
    pub sandbox: Sandbox,
    pub workspace: Workspace,
    pub worktree: Worktree,
}

impl PublicationParents {
    fn revisions(&self) -> AgentWorkPublicationParents {
        AgentWorkPublicationParents {
            sandbox_revision: self.sandbox.revision,
            workspace_revision: self.workspace.revision,
            worktree_revision: self.worktree.revision,
            lineage: AgentWorkPublicationLineage::Authorized,
        }
    }

    fn compatible(&self, current: &Self, agent: &Agent) -> bool {
        if validate_lineage(
            agent,
            &current.sandbox,
            &current.workspace,
            &current.worktree,
        )
        .is_err()
            || current.sandbox.revision < self.sandbox.revision
            || current.workspace.revision < self.workspace.revision
            || current.workspace.next_worktree_number < self.workspace.next_worktree_number
            || current.sandbox.retirement_archive_pending()
            || matches!(
                current.sandbox.status,
                SandboxStatus::Retiring | SandboxStatus::Retired
            )
            || matches!(
                current.workspace.status,
                WorkspaceStatus::Removing | WorkspaceStatus::Removed
            )
            || matches!(
                current.worktree.status,
                WorktreeStatus::Removing | WorktreeStatus::Removed
            )
        {
            return false;
        }
        let mut normalized = current.clone();
        normalized.sandbox.revision = self.sandbox.revision;
        normalized.sandbox.updated_at = self.sandbox.updated_at.clone();
        normalized.sandbox.last_activity_at = self.sandbox.last_activity_at.clone();
        normalized.workspace.revision = self.workspace.revision;
        normalized.workspace.next_worktree_number = self.workspace.next_worktree_number;
        normalized == *self
    }
}

impl AgentWorker {
    async fn publication_parents(
        &self,
        agent: &Agent,
    ) -> Result<Option<PublicationParents>, SandboxStoreError> {
        let (sandbox, workspace, worktree) = tokio::join!(
            self.dependencies.store.get_sandbox(&agent.sandbox_id),
            self.dependencies.store.get_workspace(&agent.workspace_id),
            self.dependencies.store.get_worktree(&agent.worktree_id),
        );
        let (Some(sandbox), Some(workspace), Some(worktree)) = (sandbox?, workspace?, worktree?)
        else {
            return Ok(None);
        };
        validate_lineage(agent, &sandbox, &workspace, &worktree)
            .map_err(|_| SandboxStoreError::Conflict)?;
        Ok(Some(PublicationParents {
            sandbox,
            workspace,
            worktree,
        }))
    }

    pub(in crate::sandbox::agent_worker) async fn publish_lifecycle_work<F, Fut>(
        &self,
        original: Agent,
        observed_parents: Option<PublicationParents>,
        now: SystemTime,
        publish: F,
    ) -> Result<(), SandboxStoreError>
    where
        F: FnMut(Agent, SystemTime, Option<AgentWorkPublicationParents>) -> Fut,
        Fut: Future<Output = Result<AgentWorkPublication, SandboxStoreError>>,
    {
        publication_result(
            self.try_publish_lifecycle_work(original, observed_parents, now, publish)
                .await?,
        )
    }

    pub(in crate::sandbox::agent_worker) async fn try_publish_lifecycle_work<F, Fut>(
        &self,
        original: Agent,
        observed_parents: Option<PublicationParents>,
        now: SystemTime,
        mut publish: F,
    ) -> Result<AgentWorkPublication, SandboxStoreError>
    where
        F: FnMut(Agent, SystemTime, Option<AgentWorkPublicationParents>) -> Fut,
        Fut: Future<Output = Result<AgentWorkPublication, SandboxStoreError>>,
    {
        let started = tokio::time::Instant::now();
        let (parents, current_parents) = match observed_parents {
            Some(parents) => {
                // A native reply retains the parents that authorized its request.
                // Fresh reads supply only revisions for those same authorities.
                let current = self
                    .publication_parents(&original)
                    .await?
                    .ok_or(SandboxStoreError::Conflict)?;
                if current != parents && !parents.compatible(&current, &original) {
                    return Err(SandboxStoreError::Conflict);
                }
                (parents, current)
            }
            None => {
                if original
                    .work
                    .cleanup_debt
                    .iter()
                    .any(|debt| debt.step == original.work.step)
                {
                    // This work has an outstanding native request. A fresh parent
                    // snapshot cannot establish which runtime authorized it.
                    return Err(SandboxStoreError::Conflict);
                }
                let Some(parents) = self.publication_parents(&original).await? else {
                    return publish(original, now, None).await;
                };
                (parents.clone(), parents)
            }
        };
        self.publish_with_parent_proof(
            original,
            current_parents.revisions(),
            Some(parents),
            now,
            started,
            publish,
        )
        .await
    }

    pub(super) async fn publish_rejected_lineage<F, Fut>(
        &self,
        original: Agent,
        rejection: AgentWorkPublicationParents,
        now: SystemTime,
        publish: F,
    ) -> Result<(), SandboxStoreError>
    where
        F: FnMut(Agent, SystemTime, Option<AgentWorkPublicationParents>) -> Fut,
        Fut: Future<Output = Result<AgentWorkPublication, SandboxStoreError>>,
    {
        // A retry must retain the exact revisions that established the rejection.
        publication_result(
            self.publish_with_parent_proof(
                original,
                rejection,
                None,
                now,
                tokio::time::Instant::now(),
                publish,
            )
            .await?,
        )
    }

    async fn publish_with_parent_proof<F, Fut>(
        &self,
        original: Agent,
        mut proof: AgentWorkPublicationParents,
        authorized_parents: Option<PublicationParents>,
        now: SystemTime,
        started: tokio::time::Instant,
        mut publish: F,
    ) -> Result<AgentWorkPublication, SandboxStoreError>
    where
        F: FnMut(Agent, SystemTime, Option<AgentWorkPublicationParents>) -> Fut,
        Fut: Future<Output = Result<AgentWorkPublication, SandboxStoreError>>,
    {
        let authority = execution_authority();
        if self.config.ownership.is_some() && authority.is_none() {
            return Err(SandboxStoreError::Conflict);
        }
        let mut current = original.clone();
        for attempt in 0..ATTEMPTS {
            let durable = layer2_core::seconds_from_rfc3339(&current.updated_at)
                .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
                .ok_or_else(|| SandboxStoreError::Corrupt("invalid Agent update time".into()))?;
            let at = (now + started.elapsed()).max(durable);
            if !same_publication_claim(&original, &current, &self.config.holder, at) {
                return Err(SandboxStoreError::Conflict);
            }
            if let Some(authority) = &authority {
                if authority.token.agent_id != original.id || !authority.is_current() {
                    return Err(SandboxStoreError::Conflict);
                }
                if self.config.ownership.is_some() {
                    authority
                        .check()
                        .await
                        .map_err(|_| SandboxStoreError::Conflict)?;
                }
            }
            match publish(current.clone(), at, Some(proof)).await? {
                AgentWorkPublication::Published => return Ok(AgentWorkPublication::Published),
                AgentWorkPublication::KnownAborted if attempt + 1 == ATTEMPTS => break,
                AgentWorkPublication::KnownAborted => {}
            }
            current = self
                .dependencies
                .store
                .get_agent(&original.id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?;
            if let Some(parents) = &authorized_parents {
                let current_parents = self
                    .publication_parents(&original)
                    .await?
                    .ok_or(SandboxStoreError::NotFound)?;
                if !parents.compatible(&current_parents, &current) {
                    return Err(SandboxStoreError::Conflict);
                }
                proof = current_parents.revisions();
            }
            tokio::task::yield_now().await;
        }
        Ok(AgentWorkPublication::KnownAborted)
    }

    pub(in crate::sandbox::agent_worker) async fn commit_lifecycle_publication(
        &self,
        mutation: AgentWorkMutation,
        parents: Option<AgentWorkPublicationParents>,
    ) -> Result<AgentWorkPublication, SandboxStoreError> {
        let store = &self.dependencies.store;
        if let Some(parents) = parents {
            return store.try_publish_agent_work(mutation, parents).await;
        }
        store.commit_agent_work(mutation).await?;
        Ok(AgentWorkPublication::Published)
    }
}

fn publication_result(result: AgentWorkPublication) -> Result<(), SandboxStoreError> {
    match result {
        AgentWorkPublication::Published => Ok(()),
        AgentWorkPublication::KnownAborted => Err(SandboxStoreError::Conflict),
    }
}

pub(in crate::sandbox::agent_worker) fn same_publication_claim(
    original: &Agent,
    current: &Agent,
    holder: &str,
    now: SystemTime,
) -> bool {
    let Some(original_lease) = original.work.lease.as_ref() else {
        return false;
    };
    let previous_preferences = &original.conversation.preferences;
    let preferences = &current.conversation.preferences;
    if current.requested_stop_reason.is_some()
        || current.deleted_at.is_some()
        || current.revision < original.revision
        || (current.revision == original.revision && current != original)
        || current.conversation.last_seq < original.conversation.last_seq
        || preferences.revision < previous_preferences.revision
        || (preferences.title != previous_preferences.title
            && preferences.revision == previous_preferences.revision)
        || preferences.validate().is_err()
        || current.work.lease.as_ref().is_none_or(|lease| {
            lease.holder != holder
                || lease.holder != original_lease.holder
                || lease.fence == 0
                || lease.fence != original_lease.fence
                || lease.expires_at < original_lease.expires_at
                || layer2_core::seconds_from_rfc3339(&lease.expires_at).is_none_or(|seconds| {
                    now >= SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
                })
        })
    {
        return false;
    }
    let mut normalized = current.clone();
    normalized.revision = original.revision;
    normalized.updated_at = original.updated_at.clone();
    normalized.title = original.title.clone();
    normalized.conversation.last_seq = original.conversation.last_seq;
    normalized.conversation.preferences.title = previous_preferences.title.clone();
    normalized.conversation.preferences.revision = previous_preferences.revision;
    normalized.work.lease = original.work.lease.clone();
    normalized == *original
}
