//! Atomic in-memory aggregate/event commits share the persisted store guards.
use super::*;

#[async_trait]
impl SandboxTransactionStore for InMemorySandboxStore {
    async fn commit(&self, transaction: SandboxTransaction) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        let mut candidate = state.clone();
        match transaction {
            SandboxTransaction::CreateWorktreeAndAgent {
                worktree,
                agent,
                event_at,
                event,
            } => {
                if agent.worktree_id != worktree.id || agent.sandbox_id != worktree.sandbox_id {
                    return Err(SandboxStoreError::InvalidTransaction("lineage mismatch"));
                }
                let sandbox_id = worktree.sandbox_id.clone();
                let worktree_id = worktree.id.clone();
                let agent_id = agent.id.clone();
                create_record!(candidate, worktrees, worktree_id, *worktree)?;
                create_record!(candidate, agents, agent_id, *agent)?;
                append_sandbox_event(&mut candidate, &sandbox_id, event_at, event)?;
            }
            SandboxTransaction::SaveSandboxAndAppend {
                command_revival,
                released_runtime,
                expected_revision,
                sandbox,
                event_at,
                event,
            } => {
                if let Some(proof) = &command_revival {
                    command_revival::validate(
                        &mut candidate,
                        proof,
                        &sandbox,
                        &event_at,
                        &event,
                        released_runtime.as_ref().and_then(|release| {
                            self.managed_runtime_target(&sandbox, &release.agent)
                        }),
                    )?;
                }
                let id = sandbox.id.clone();
                if candidate.sandboxes.get(&id).is_some_and(|current| {
                    current.owner != sandbox.owner
                        || current
                            .validate_retirement_archive_successor(&sandbox)
                            .is_err()
                        || current
                            .validate_unpublished_recovery_save(&sandbox)
                            .is_err()
                        || current
                            .validate_service_access_transaction(&sandbox, &event, &event_at)
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
                save_record!(
                    candidate,
                    sandboxes,
                    id.clone(),
                    expected_revision,
                    *sandbox
                )?;
                append_sandbox_event(&mut candidate, &id, event_at, event)?;
                if let Some(proof) = command_revival {
                    proof
                        .require_current()
                        .map_err(|_| SandboxStoreError::Conflict)?;
                }
            }
            SandboxTransaction::SaveWorkspaceAndAppend {
                expected_revision,
                workspace,
                event_at,
                event,
                ..
            } => {
                let id = workspace.id.clone();
                let sandbox_id = workspace.sandbox_id.clone();
                if candidate.workspaces.get(&id).is_some_and(|current| {
                    crate::sandbox::ProvisioningExecution::validate_successor(
                        current.work.execution,
                        workspace.work.execution,
                    )
                    .is_err()
                }) {
                    return Err(SandboxStoreError::Conflict);
                }
                save_record!(candidate, workspaces, id, expected_revision, *workspace)?;
                append_sandbox_event(&mut candidate, &sandbox_id, event_at, event)?;
            }
        }
        *state = candidate;
        Ok(())
    }
}
