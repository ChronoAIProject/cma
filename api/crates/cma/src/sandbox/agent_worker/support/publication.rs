//! Reattempt only a definitively aborted publication of the original work claim.
use std::future::Future;
use std::time::{Duration, SystemTime};

use layer2_core::agent_ownership::execution_authority;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, Sandbox, Workspace, Worktree};
use layer3_adapter::owner_protocol::TransportAuthority;

use super::{validate_lineage, AgentContext, AgentWorker};

const MAX_ATTEMPTS: usize = 8;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::sandbox::agent_worker) struct PublicationParents {
    sandbox: Option<Sandbox>,
    workspace: Option<Workspace>,
    worktree: Option<Worktree>,
}

impl PublicationParents {
    pub(in crate::sandbox::agent_worker) fn from_context(context: &AgentContext) -> Self {
        Self::from_records(&context.sandbox, &context.workspace, &context.worktree)
    }

    pub(in crate::sandbox::agent_worker) fn from_records(
        sandbox: &Sandbox,
        workspace: &Workspace,
        worktree: &Worktree,
    ) -> Self {
        Self {
            sandbox: Some(sandbox.clone()),
            workspace: Some(workspace.clone()),
            worktree: Some(worktree.clone()),
        }
    }

    fn compatible(&self, current: &Self, agent: &Agent) -> bool {
        let (Some(sandbox), Some(workspace), Some(worktree)) =
            (&current.sandbox, &current.workspace, &current.worktree)
        else {
            return false;
        };
        if validate_lineage(agent, sandbox, workspace, worktree).is_err() {
            return false;
        }
        let mut normalized = current.clone();
        if let (Some(previous), Some(next)) = (&self.sandbox, &mut normalized.sandbox) {
            if next.revision < previous.revision {
                return false;
            }
            next.revision = previous.revision;
            next.updated_at = previous.updated_at.clone();
            next.last_activity_at = previous.last_activity_at.clone();
        }
        if let (Some(previous), Some(next)) = (&self.workspace, &mut normalized.workspace) {
            if next.revision < previous.revision
                || next.next_worktree_number < previous.next_worktree_number
            {
                return false;
            }
            next.revision = previous.revision;
            next.next_worktree_number = previous.next_worktree_number;
        }
        if let (Some(previous), Some(next)) = (&self.worktree, &mut normalized.worktree) {
            if next.revision < previous.revision {
                return false;
            }
            next.revision = previous.revision;
        }
        normalized == *self
    }
}

impl AgentWorker {
    async fn publication_parents(
        &self,
        agent: &Agent,
    ) -> Result<PublicationParents, SandboxStoreError> {
        let (sandbox, workspace, worktree) = tokio::join!(
            self.dependencies.store.get_sandbox(&agent.sandbox_id),
            self.dependencies.store.get_workspace(&agent.workspace_id),
            self.dependencies.store.get_worktree(&agent.worktree_id),
        );
        Ok(PublicationParents {
            sandbox: sandbox?,
            workspace: workspace?,
            worktree: worktree?,
        })
    }

    // Callers retain the native outcome outside this closure. Never enclose
    // preparation, transport, installation, or an external cleanup operation.
    pub(in crate::sandbox::agent_worker) async fn retry_work_publication<F, Fut, T>(
        &self,
        original: Agent,
        observed: Option<&PublicationParents>,
        now: SystemTime,
        mut publish: F,
    ) -> Result<T, SandboxStoreError>
    where
        F: FnMut(Agent, SystemTime) -> Fut,
        Fut: Future<Output = Result<T, SandboxStoreError>>,
    {
        let started = tokio::time::Instant::now();
        let parents = self.publication_parents(&original).await?;
        // Native callers supply the parents observed before their one effect.
        if observed.is_some_and(|prior| !prior.compatible(&parents, &original)) {
            return Err(SandboxStoreError::Conflict);
        }
        let authority = execution_authority();
        if self.config.ownership.is_some() && authority.is_none() {
            return Err(SandboxStoreError::Conflict);
        }
        let mut current = original.clone();
        for attempt in 0..MAX_ATTEMPTS {
            let durable_at = layer2_core::seconds_from_rfc3339(&current.updated_at)
                .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
                .ok_or_else(|| SandboxStoreError::Corrupt("invalid agent update time".into()))?;
            let at = (now + started.elapsed()).max(durable_at);
            if !same_publication_claim(&original, &current, &self.config.holder, at) {
                return Err(SandboxStoreError::Conflict);
            }
            if let Some(authority) = &authority {
                if authority.token.agent_id != original.id || !authority.is_current() {
                    return Err(SandboxStoreError::Conflict);
                }
                // Use the captured execution authority, never acquire or borrow
                // whichever owner's epoch happens to be current after a race.
                if self.config.ownership.is_some() {
                    authority
                        .check()
                        .await
                        .map_err(|_| SandboxStoreError::Conflict)?;
                }
            }
            match publish(current.clone(), at).await {
                Err(SandboxStoreError::Conflict) if attempt + 1 < MAX_ATTEMPTS => {
                    current = self
                        .dependencies
                        .store
                        .get_agent(&original.id)
                        .await?
                        .ok_or(SandboxStoreError::NotFound)?;
                    let fresh = self.publication_parents(&original).await?;
                    if !parents.compatible(&fresh, &current) {
                        return Err(SandboxStoreError::Conflict);
                    }
                    tokio::task::yield_now().await;
                }
                // Unavailable includes unknown commit. Its original receipt
                // reconciliation stays in the store; it never authorizes replay.
                result => return result,
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

pub(in crate::sandbox::agent_worker) fn same_publication_claim(
    original: &Agent,
    current: &Agent,
    holder: &str,
    now: SystemTime,
) -> bool {
    if current.revision < original.revision
        || (current.revision == original.revision && current != original)
        || current.conversation.last_seq < original.conversation.last_seq
        || current.work.lease.as_ref().is_none_or(|lease| {
            lease.holder != holder
                || lease.fence == 0
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
    normalized == *original
}
