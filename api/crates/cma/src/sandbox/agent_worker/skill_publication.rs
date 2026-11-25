use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::agent_ownership::{execution_authority, Authority};
use layer2_core::sandbox::store::{AgentSkillResultGuard, SandboxStoreError};
use layer2_core::sandbox::{
    Agent, AgentSkillInstallStatus, AgentStatus, AgentWorkStep, Sandbox, Workspace, Worktree,
};
use layer2_core::seconds_from_rfc3339;
use layer3_adapter::owner_protocol::TransportAuthority;

use super::super::support::{validate_lineage, AgentContext};
use super::AgentWorker;

const MAX_ATTEMPTS: usize = 8;

// Captured before installation; a late result cannot borrow a successor's intent
// or execution authority. The entire Agent pins material and request history.
pub(super) struct SkillPublication {
    original: Agent,
    parents: Parents,
    authority: Option<Arc<Authority>>,
    guard: AgentSkillResultGuard,
}

impl SkillPublication {
    pub(super) fn capture(
        agent: &Agent,
        context: &AgentContext,
    ) -> Result<Self, SandboxStoreError> {
        Ok(Self {
            original: agent.clone(),
            parents: Parents {
                sandbox: context.sandbox.clone(),
                workspace: context.workspace.clone(),
                worktree: context.worktree.clone(),
            },
            authority: execution_authority(),
            guard: AgentSkillResultGuard::capture(
                agent,
                &context.sandbox,
                &context.workspace,
                &context.worktree,
            )?,
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Parents {
    sandbox: Sandbox,
    workspace: Workspace,
    worktree: Worktree,
}

impl Parents {
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
            || current.worktree.revision < self.worktree.revision
        {
            return false;
        }
        let mut normalized = current.clone();
        normalized.sandbox.revision = self.sandbox.revision;
        normalized.sandbox.updated_at = self.sandbox.updated_at.clone();
        normalized.sandbox.last_activity_at = self.sandbox.last_activity_at.clone();
        normalized.workspace.revision = self.workspace.revision;
        normalized.workspace.next_worktree_number = self.workspace.next_worktree_number;
        normalized.worktree.revision = self.worktree.revision;
        normalized == *self
    }
}

impl AgentWorker {
    pub(super) async fn publish_skill_result(
        &self,
        observed: SkillPublication,
        error: Option<&str>,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        if self.config.ownership.is_some() && observed.authority.is_none() {
            return Err(SandboxStoreError::Conflict);
        }
        for attempt in 0..MAX_ATTEMPTS {
            let current = self
                .dependencies
                .store
                .get_agent(&observed.original.id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?;
            let (sandbox, workspace, worktree) = tokio::join!(
                self.dependencies
                    .store
                    .get_sandbox(&observed.original.sandbox_id),
                self.dependencies
                    .store
                    .get_workspace(&observed.original.workspace_id),
                self.dependencies
                    .store
                    .get_worktree(&observed.original.worktree_id),
            );
            let parents = Parents {
                sandbox: sandbox?.ok_or(SandboxStoreError::NotFound)?,
                workspace: workspace?.ok_or(SandboxStoreError::NotFound)?,
                worktree: worktree?.ok_or(SandboxStoreError::NotFound)?,
            };
            if !observed.parents.compatible(&parents, &current) {
                return Err(SandboxStoreError::Conflict);
            }
            if let Some(authority) = &observed.authority {
                if authority.token.agent_id != observed.original.id || !authority.is_current() {
                    return Err(SandboxStoreError::Conflict);
                }
                if self.config.ownership.is_some() {
                    authority
                        .check()
                        .await
                        .map_err(|_| SandboxStoreError::Conflict)?;
                }
            }
            let durable_at = seconds_from_rfc3339(&current.updated_at)
                .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
                .ok_or_else(|| SandboxStoreError::Corrupt("invalid agent update time".into()))?;
            let at = (now + started.elapsed()).max(durable_at);
            if !same_claim(&observed.original, &current, &self.config.holder, at) {
                return Err(SandboxStoreError::Conflict);
            }
            // This loop owns only the fixed native result. Conflict denotes a
            // known abort; unknown commit/Unavailable stays with store receipt
            // reconciliation and never authorizes a fresh publication proposal.
            match self.finish_skill(current, error, &observed.guard, at).await {
                Err(SandboxStoreError::Conflict) if attempt + 1 < MAX_ATTEMPTS => {
                    tokio::task::yield_now().await;
                }
                result => return result,
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

fn same_claim(original: &Agent, current: &Agent, holder: &str, now: SystemTime) -> bool {
    if current.status != AgentStatus::Ready
        || current.work.step != AgentWorkStep::InstallSkill
        || current.requested_stop_reason.is_some()
        || current.deleted_at.is_some()
        || current.work.live_skills.last().is_none_or(|skill| {
            skill.status != AgentSkillInstallStatus::Pending || skill.material.is_none()
        })
        || current.revision < original.revision
        || (current.revision == original.revision && current != original)
        || current.conversation.last_seq < original.conversation.last_seq
        || current.conversation.preferences.revision < original.conversation.preferences.revision
        || (current.conversation.preferences.title != original.conversation.preferences.title
            && current.conversation.preferences.revision
                == original.conversation.preferences.revision)
        || current.conversation.preferences.validate().is_err()
        || current.work.lease.as_ref().is_none_or(|lease| {
            lease.holder != holder
                || lease.fence == 0
                || seconds_from_rfc3339(&lease.expires_at).is_none_or(|seconds| {
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
    normalized.conversation.preferences.title = original.conversation.preferences.title.clone();
    normalized.conversation.preferences.revision = original.conversation.preferences.revision;
    normalized == *original
}
