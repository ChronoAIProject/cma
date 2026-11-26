use crate::sandbox::{SandboxOrigin, SandboxStatus};
use async_trait::async_trait;

use super::*;

#[macro_use]
mod macros;

mod admission;
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
mod agent_log;
mod agent_reads;
mod agent_worktree_recovery;
#[cfg(test)]
mod agent_worktree_recovery_tests;
mod binding_reset;
mod bindings;
mod capacity;
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
mod custom_runtime;
use custom_runtime::validate_new_sandbox;
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
#[cfg(test)]
mod retained_recovery_tests;
mod retained_reopen;
#[cfg(test)]
mod retained_reopen_tests;
mod runtime_activity;
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
    async fn owner_capacity(
        &self,
        owner: &SubjectId,
    ) -> Result<crate::sandbox::SandboxCapacitySnapshot, SandboxStoreError> {
        capacity::read(self, owner)
    }

    async fn failed_lifecycle_admission(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<crate::sandbox::SandboxLifecycleAdmission>, SandboxStoreError> {
        use crate::sandbox::SandboxLifecycleAdmission;
        let state = self.lock();
        if state.sandboxes.get(&sandbox.id) != Some(sandbox) {
            return Ok(None);
        }
        let transitions = state
            .sandbox_events
            .get(&sandbox.id)
            .into_iter()
            .flatten()
            .rev()
            .filter(|event| SandboxLifecycleAdmission::is_boundary(event))
            .take(3)
            .cloned()
            .collect::<Vec<_>>();
        let controls = state
            .sandbox_controls
            .values()
            .filter(|record| record.envelope.sandbox_id == sandbox.id)
            .cloned()
            .collect::<Vec<_>>();
        Ok(SandboxLifecycleAdmission::from_history(
            sandbox,
            &transitions,
            &controls,
        ))
    }

    async fn retirement_admitted_at(
        &self,
        id: &SandboxId,
    ) -> Result<Option<String>, SandboxStoreError> {
        let state = self.lock();
        let event = state
            .sandbox_events
            .get(id)
            .into_iter()
            .flatten()
            .find(|event| {
                matches!(
                    event.kind,
                    SandboxEventKind::StatusChanged {
                        to: SandboxStatus::Retiring,
                        ..
                    }
                )
            })
            .map(|event| event.appended_at.clone());
        Ok(event.map(|at| {
            state.sandbox_controls.values().find(|record| {
                record.envelope.sandbox_id == *id
                    && record.envelope.command == crate::sandbox::control::SandboxControl::Retire
                    && matches!(&record.status, crate::sandbox::control::SandboxControlStatus::Applied {
                        at: applied, disposition: crate::sandbox::control::SandboxControlDisposition::Scheduled, ..
                    } if applied == &at)
            }).map_or(at, |record| record.admitted_at.clone())
        }))
    }
    async fn retained_reopen_snapshot(
        &self,
        id: &SandboxId,
    ) -> Result<crate::sandbox::RetainedReopenSnapshot, SandboxStoreError> {
        retained_reopen::snapshot(&self.lock(), id)
    }

    async fn retained_reopen_receipt(
        &self,
        id: &SandboxId,
    ) -> Result<Option<crate::sandbox::RetainedReopenReceipt>, SandboxStoreError> {
        Ok(self
            .lock()
            .retained_reopen_receipts
            .get(id)
            .and_then(|receipts| receipts.first())
            .cloned())
    }

    async fn retained_reopen_history(
        &self,
        id: &SandboxId,
        key_sha256: Option<&str>,
    ) -> Result<crate::sandbox::RetainedReopenHistory, SandboxStoreError> {
        let state = self.lock();
        let parent = state.sandboxes.get(id).ok_or(SandboxStoreError::NotFound)?;
        let observed_parent = Some((parent.owner.clone(), parent.revision));
        let Some(receipts) = state.retained_reopen_receipts.get(id) else {
            return Ok(crate::sandbox::RetainedReopenHistory {
                observed_parent,
                ..Default::default()
            });
        };
        Ok(crate::sandbox::RetainedReopenHistory {
            observed_parent,
            latest_revision: receipts.last().map(|receipt| receipt.accepted.revision),
            replay: receipts
                .iter()
                .find(|receipt| key_sha256 == Some(receipt.key_sha256.as_str()))
                .cloned()
                .map(Into::into),
        })
    }

    async fn admit_retained_reopen(
        &self,
        request: crate::sandbox::RetainedReopenRequest,
    ) -> Result<crate::sandbox::RetainedReopenAccepted, SandboxStoreError> {
        retained_reopen::admit(&mut self.lock(), request)
    }

    async fn retained_runtime_recovery_snapshot(
        &self,
        id: &SandboxId,
    ) -> Result<crate::sandbox::RetainedRecoverySnapshot, SandboxStoreError> {
        unpublished_recovery::retained_snapshot(&self.lock(), id)
    }

    fn managed_runtime_version(&self) -> Option<String> {
        self.managed_runtime_version.clone()
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
        validate_new_sandbox(&sandbox)?;
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
                || current.lifecycle.custom_runtime != sandbox.lifecycle.custom_runtime
                || current.lifecycle.runtime_migration_admission
                    != sandbox.lifecycle.runtime_migration_admission
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
            || current.conversation.turn_timeout != agent.conversation.turn_timeout
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

mod sandbox_events;

impl InMemorySandboxStore {
    pub fn with_managed_runtime_version(mut self, version: String) -> Self {
        self.managed_runtime_version = Some(version);
        self
    }
}
