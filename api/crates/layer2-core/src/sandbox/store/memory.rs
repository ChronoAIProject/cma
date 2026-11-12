use crate::sandbox::{SandboxOrigin, SandboxStatus};
use async_trait::async_trait;

use super::*;

#[macro_use]
mod record_macros;

mod agent_admission;
#[cfg(test)]
mod agent_admission_tests;
mod agent_artifacts;
mod agent_command_work;
mod agent_commands;
#[cfg(test)]
mod agent_connection_recovery_tests;
mod agent_control;
#[cfg(test)]
mod agent_control_tests;
mod agent_conversation;
#[cfg(test)]
mod agent_conversation_tests;
mod agent_lifecycle;
#[cfg(test)]
mod agent_lifecycle_tests;
mod agent_reads;
mod agent_worktree_recovery;
#[cfg(test)]
mod agent_worktree_recovery_tests;
mod bindings;
mod channel_usage;
mod command_audit;
mod command_revival;
mod events;
mod interrupting_messages;
mod provenance;
mod provenance_inventory;
mod provider_associations;
mod queued_cancellations;
#[cfg(test)]
mod queued_cancellations_tests;
mod queued_inputs;
#[cfg(test)]
mod queued_inputs_tests;
mod suggestions;
use events::{append_sandbox_event, bounded_events};
#[cfg(test)]
mod launch_command_tests;
mod lifecycle;
#[cfg(test)]
mod lifecycle_command_tests;
mod lifecycle_execution;
#[cfg(test)]
mod lifecycle_execution_tests;
#[cfg(test)]
mod lifecycle_validation_tests;
mod maintenance;
#[cfg(test)]
mod maintenance_tests;
mod reconciliation;
#[cfg(test)]
mod reconciliation_tests;
mod resource_access;
mod sandbox_controls;
mod sessions;
mod targeted_controls;
#[cfg(test)]
mod targeted_controls_tests;
mod transactions;
mod unpublished_recovery;
#[cfg(test)]
mod unpublished_recovery_tests;
mod workspace_lifecycle;

#[async_trait]
impl SandboxStore for InMemorySandboxStore {
    fn managed_runtime_version(&self) -> Option<&str> {
        self.managed_runtime_version.as_deref()
    }

    async fn unpublished_runtime_recovery_snapshot(
        &self,
        id: &SandboxId,
    ) -> Result<crate::sandbox::SandboxUnpublishedRecoverySnapshot, SandboxStoreError> {
        unpublished_recovery::snapshot(&self.lock(), id)
    }

    async fn admit_unpublished_runtime_recovery(
        &self,
        request: crate::sandbox::SandboxUnpublishedRecoveryRequest,
    ) -> Result<Sandbox, SandboxStoreError> {
        unpublished_recovery::admit(&mut self.lock(), request)
    }

    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        if !sandbox.credential.service_grants.is_empty()
            || sandbox.credential.service_scope.is_some()
        {
            return Err(SandboxStoreError::InvalidTransaction(
                "service access requires an existing sandbox and an atomic grant",
            ));
        }
        if sandbox.origin.replay_key().is_some() {
            return Err(SandboxStoreError::InvalidTransaction(
                "origin-bound sandboxes require atomic admission",
            ));
        }
        let mut state = self.lock();
        if state.sandboxes.values().any(|stored| {
            stored.owner == sandbox.owner
                && stored.name == sandbox.name
                && stored.retired_at.is_none()
        }) {
            return Err(SandboxStoreError::Duplicate);
        }
        let id = sandbox.id.clone();
        create_record!(state, sandboxes, id, sandbox)
    }

    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        Ok(self.lock().sandboxes.get(id).cloned())
    }

    async fn save_sandbox(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
    ) -> Result<(), SandboxStoreError> {
        let id = sandbox.id.clone();
        let mut state = self.lock();
        if state.sandboxes.get(&id).is_some_and(|current| {
            current.owner != sandbox.owner
                || current
                    .validate_retirement_archive_successor(&sandbox)
                    .is_err()
                || current.validate_service_access_successor(&sandbox).is_err()
                || current
                    .validate_unpublished_recovery_save(&sandbox)
                    .is_err()
                || crate::sandbox::ProvisioningExecution::validate_successor(
                    current.work.execution,
                    sandbox.work.execution,
                )
                .is_err()
                || current
                    .validate_checkpoint_capture_successor(&sandbox)
                    .is_err()
                || current.origin != sandbox.origin
                || current
                    .validate_runtime_generation_successor(&sandbox)
                    .is_err()
        }) {
            return Err(SandboxStoreError::Conflict);
        }
        save_record!(state, sandboxes, id, expected_revision, sandbox)
    }

    async fn list_sandboxes(
        &self,
        owner: &SubjectId,
        request: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        let values = self
            .lock()
            .sandboxes
            .values()
            .filter(|sandbox| &sandbox.owner == owner)
            .map(|sandbox| (sandbox.id.to_string(), sandbox.clone()))
            .collect();
        page(values, request)
    }
}

#[async_trait]
impl WorkspaceStore for InMemorySandboxStore {
    async fn create_workspace(&self, workspace: Workspace) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        if state.workspaces.values().any(|stored| {
            stored.sandbox_id == workspace.sandbox_id
                && stored.name == workspace.name
                && stored.removed_at.is_none()
        }) {
            return Err(SandboxStoreError::Duplicate);
        }
        let id = workspace.id.clone();
        create_record!(state, workspaces, id, workspace)
    }

    async fn get_workspace(
        &self,
        id: &WorkspaceId,
    ) -> Result<Option<Workspace>, SandboxStoreError> {
        Ok(self.lock().workspaces.get(id).cloned())
    }

    async fn save_workspace(
        &self,
        expected_revision: u64,
        workspace: Workspace,
    ) -> Result<(), SandboxStoreError> {
        let id = workspace.id.clone();
        let mut state = self.lock();
        if state.workspaces.get(&id).is_some_and(|current| {
            crate::sandbox::ProvisioningExecution::validate_successor(
                current.work.execution,
                workspace.work.execution,
            )
            .is_err()
        }) {
            return Err(SandboxStoreError::Conflict);
        }
        save_record!(state, workspaces, id, expected_revision, workspace)
    }

    async fn list_workspaces(
        &self,
        sandbox_id: &SandboxId,
        request: &PageRequest,
    ) -> Result<Page<Workspace>, SandboxStoreError> {
        let values = self
            .lock()
            .workspaces
            .values()
            .filter(|workspace| &workspace.sandbox_id == sandbox_id)
            .map(|workspace| (workspace.id.to_string(), workspace.clone()))
            .collect();
        page(values, request)
    }
}

#[async_trait]
impl WorktreeStore for InMemorySandboxStore {
    async fn create_worktree(&self, worktree: Worktree) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        if state.worktrees.values().any(|stored| {
            stored.workspace_id == worktree.workspace_id && stored.name == worktree.name
        }) {
            return Err(SandboxStoreError::Duplicate);
        }
        let id = worktree.id.clone();
        create_record!(state, worktrees, id, worktree)
    }

    async fn get_worktree(&self, id: &WorktreeId) -> Result<Option<Worktree>, SandboxStoreError> {
        Ok(self.lock().worktrees.get(id).cloned())
    }

    async fn save_worktree(
        &self,
        expected_revision: u64,
        worktree: Worktree,
    ) -> Result<(), SandboxStoreError> {
        let id = worktree.id.clone();
        let mut state = self.lock();
        save_record!(state, worktrees, id, expected_revision, worktree)
    }

    async fn list_worktrees(
        &self,
        workspace_id: &WorkspaceId,
        request: &PageRequest,
    ) -> Result<Page<Worktree>, SandboxStoreError> {
        let values = self
            .lock()
            .worktrees
            .values()
            .filter(|worktree| &worktree.workspace_id == workspace_id)
            .map(|worktree| (worktree.id.to_string(), worktree.clone()))
            .collect();
        page(values, request)
    }
}

#[async_trait]
impl AgentStore for InMemorySandboxStore {
    async fn require_legacy_first_message(&self, agent: &Agent) -> Result<(), SandboxStoreError> {
        let current = self
            .get_agent(&agent.id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if current
            .conversation
            .first_message
            .as_ref()
            .is_some_and(|first| first.common_binding.is_some())
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }

    async fn create_agent(&self, agent: Agent) -> Result<(), SandboxStoreError> {
        if agent.work.resolved_skills.is_some()
            || !agent.work.live_skills.is_empty()
            || !agent.work.worktree_recoveries.is_empty()
        {
            return Err(SandboxStoreError::Conflict);
        }
        let mut state = self.lock();
        if state.agents.values().any(|stored| {
            stored.worktree_id == agent.worktree_id
                && !matches!(stored.status, super::super::AgentStatus::Deleted)
        }) {
            return Err(SandboxStoreError::Duplicate);
        }
        let id = agent.id.clone();
        create_record!(state, agents, id, agent)
    }

    async fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>, SandboxStoreError> {
        Ok(self.lock().agents.get(id).cloned())
    }

    async fn save_agent(
        &self,
        expected_revision: u64,
        agent: Agent,
    ) -> Result<(), SandboxStoreError> {
        let id = agent.id.clone();
        let mut state = self.lock();
        let current = state.agents.get(&id).ok_or(SandboxStoreError::NotFound)?;
        if current.worktree_id != agent.worktree_id
            || current.work.worktree_recoveries != agent.work.worktree_recoveries
            || current.owner != agent.owner
            || current.profile != agent.profile
            || current.conversation.settings != agent.conversation.settings
            || current.work.resolved_skills != agent.work.resolved_skills
            || current.work.live_skills != agent.work.live_skills
        {
            return Err(SandboxStoreError::Conflict);
        }
        agent
            .work
            .validate_resolved_skills(&agent.profile.content.skills)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        save_record!(state, agents, id, expected_revision, agent)
    }

    async fn list_agents(
        &self,
        owner: &SubjectId,
        request: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        let values = self
            .lock()
            .agents
            .values()
            .filter(|agent| &agent.owner == owner)
            .map(|agent| (agent.id.to_string(), agent.clone()))
            .collect();
        page(values, request)
    }
}

#[async_trait]
impl SandboxEventStore for InMemorySandboxStore {
    async fn append_sandbox_event(
        &self,
        sandbox_id: &SandboxId,
        appended_at: String,
        kind: SandboxEventKind,
    ) -> Result<SandboxEvent, SandboxStoreError> {
        let mut state = self.lock();
        if !state.sandboxes.contains_key(sandbox_id) {
            return Err(SandboxStoreError::NotFound);
        }
        append_sandbox_event(&mut state, sandbox_id, appended_at, kind)
    }

    async fn sandbox_events(
        &self,
        sandbox_id: &SandboxId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<SandboxEvent>, SandboxStoreError> {
        bounded_events(
            self.lock().sandbox_events.get(sandbox_id),
            after,
            limit,
            |event| event.seq,
        )
    }
}

#[async_trait]
impl AgentLogStore for InMemorySandboxStore {
    async fn append_agent_log(
        &self,
        agent_id: &AgentId,
        appended_at: String,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        let mut state = self.lock();
        if !state.agents.contains_key(agent_id) {
            return Err(SandboxStoreError::NotFound);
        }
        let log = state.agent_logs.entry(agent_id.clone()).or_default();
        let seq = u64::try_from(log.len())
            .map_err(|_| SandboxStoreError::SequenceConflict)?
            .checked_add(1)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        let event = AgentLogEntry {
            agent_id: agent_id.clone(),
            seq,
            appended_at,
            kind,
        };
        log.push(event.clone());
        suggestions::enqueue(&mut state, agent_id);
        Ok(event)
    }

    async fn agent_log(
        &self,
        agent_id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        let rows = bounded_events(
            self.lock().agent_logs.get(agent_id),
            after,
            limit,
            |event| event.seq,
        )?;
        #[cfg(test)]
        self.log_rows_read
            .fetch_add(rows.len(), std::sync::atomic::Ordering::SeqCst);
        Ok(rows)
    }

    async fn agent_log_range(
        &self,
        agent_id: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE || after > through {
            return Err(SandboxStoreError::InvalidPage);
        }
        let state = self.lock();
        let agent = state
            .agents
            .get(agent_id)
            .ok_or(SandboxStoreError::NotFound)?;
        let log = state
            .agent_logs
            .get(agent_id)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        if u64::try_from(log.len()).ok() != Some(agent.conversation.last_seq)
            || log.iter().enumerate().any(|(offset, entry)| {
                entry.agent_id != *agent_id || entry.seq != offset as u64 + 1
            })
            || through > agent.conversation.last_seq
        {
            return Err(SandboxStoreError::SequenceConflict);
        }
        let expected = (through - after).min(u64::from(limit)) as usize;
        Ok(log
            .iter()
            .skip(after as usize)
            .take(expected)
            .cloned()
            .collect())
    }
}

impl InMemorySandboxStore {
    pub fn with_managed_runtime_version(mut self, version: String) -> Self {
        self.managed_runtime_version = Some(version);
        self
    }
}
