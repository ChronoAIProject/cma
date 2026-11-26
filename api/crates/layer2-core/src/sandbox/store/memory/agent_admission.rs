use super::*;
use crate::sandbox::store::{AgentAdmission, AgentAdmissionResult, AgentAdmissionStore};
use crate::sandbox::{AgentOrigin, SandboxEventKind, SandboxReviveCause, WorkspaceStatus};

#[async_trait]
impl AgentAdmissionStore for InMemorySandboxStore {
    async fn replay_agent_admission(
        &self,
        admission: &AgentAdmission,
    ) -> Result<Option<AgentAdmissionResult>, SandboxStoreError> {
        replay_agent_admission(&self.lock(), admission)
    }

    async fn admit_agent_with_trigger_initiator(
        &self,
        admission: AgentAdmission,
        initiation: crate::trigger_domain::LaunchInitiation,
        reuse_existing: bool,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        self.admit_agent_with_reuse(admission, reuse_existing, Some(initiation))
            .await
    }

    async fn admit_agent_reusing_profile(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        self.admit_agent_with_reuse(admission, true, None).await
    }

    async fn admit_agent(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        self.admit_agent_with_reuse(admission, false, None).await
    }
}

impl InMemorySandboxStore {
    async fn admit_agent_with_reuse(
        &self,
        admission: AgentAdmission,
        reuse_existing: bool,
        initiation: Option<crate::trigger_domain::LaunchInitiation>,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        if let Some(initiation) = &initiation {
            initiation.require_agent_admission(&admission)?;
        }
        let mut state = self.lock();
        if let Some(fence) = &admission.binding {
            let binding = state
                .bindings
                .get(&fence.binding_id)
                .ok_or(SandboxStoreError::Conflict)?;
            let proof = fence
                .conversation
                .as_ref()
                .ok_or(SandboxStoreError::Conflict)?;
            let key = format!(
                "{}:{:020}:{}",
                fence.binding_id, fence.generation, proof.conversation_id
            );
            let conversation = state
                .binding_conversations
                .get(&key)
                .ok_or(SandboxStoreError::Conflict)?;
            fence.require_agent(binding, conversation, &admission)?;
        }
        if let Some(replay) = replay_agent_admission(&state, &admission)? {
            return Ok(replay);
        }

        let workspace = state
            .workspaces
            .get(&admission.workspace_id)
            .cloned()
            .ok_or(SandboxStoreError::WorkspaceNotReady)?;
        let sandbox = state
            .sandboxes
            .get(&workspace.sandbox_id)
            .cloned()
            .ok_or(SandboxStoreError::SandboxNotReady)?;
        let bootstrap = admission.admits_bootstrap(&workspace, &sandbox);
        if workspace.owner != admission.owner
            || workspace.owner != sandbox.owner
            || workspace.sandbox_id != sandbox.id
            || (!bootstrap && workspace.status != WorkspaceStatus::Ready)
        {
            return Err(SandboxStoreError::WorkspaceNotReady);
        }
        if !bootstrap && !sandbox.accepts_creation_admission() {
            return Err(SandboxStoreError::SandboxNotReady);
        }
        super::admission::require_operator_open(&state, admission.admission_cause())?;
        if let Some(mut agent) = state
            .agents
            .values()
            .filter(|agent| admission.can_reuse(agent, &workspace, reuse_existing))
            .filter(|agent| {
                reuse_existing
                    || !admission.claims_unused_prepared_agent(agent)
                    || (!state
                        .agent_reuse_bindings
                        .values()
                        .any(|(id, _)| id == &agent.id)
                        && state.agent_logs.get(&agent.id).is_some_and(|logs| {
                            !logs.iter().any(|entry| {
                                matches!(entry.kind, AgentLogKind::OwnerMessage { .. })
                            })
                        }))
            })
            .max_by(|a, b| (&a.created_at, &a.id).cmp(&(&b.created_at, &b.id)))
            .cloned()
        {
            admission.materialize(&workspace)?;
            let logs = state
                .agent_logs
                .get(&agent.id)
                .ok_or_else(|| SandboxStoreError::Corrupt("reused Agent has no log".into()))?;
            let initial_log = logs.first().cloned().ok_or_else(|| {
                SandboxStoreError::Corrupt("reused Agent has no creation log".into())
            })?;
            let has_input = logs
                .iter()
                .any(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }));
            let mut candidate = state.clone();
            if let Some(kind) = admission.initialize_reused_agent(&mut agent, has_input)? {
                candidate
                    .agent_logs
                    .get_mut(&agent.id)
                    .expect("validated log")
                    .push(AgentLogEntry {
                        agent_id: agent.id.clone(),
                        seq: agent.conversation.last_seq,
                        appended_at: admission.event_at.clone(),
                        kind,
                    });
                candidate.agents.insert(agent.id.clone(), agent.clone());
            }
            if matches!(admission.origin, AgentOrigin::Trigger { .. }) {
                candidate.agent_reuse_bindings.insert(
                    (admission.owner.clone(), admission.creation_key.clone()),
                    (agent.id.clone(), admission.fingerprint()?),
                );
                let mut next_sandbox = sandbox.clone();
                let revival = next_sandbox
                    .record_creation_admission_with_managed_runtime(
                        SandboxReviveCause::TriggerLaunch,
                        self.managed_runtime_version().as_deref(),
                        admission.event_at.clone(),
                    )
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
                candidate
                    .sandboxes
                    .insert(next_sandbox.id.clone(), next_sandbox);
                if let Some(event) = revival {
                    append_sandbox_event(
                        &mut candidate,
                        &sandbox.id,
                        admission.event_at.clone(),
                        event,
                    )?;
                }
            }
            let worktree = candidate
                .worktrees
                .get(&agent.worktree_id)
                .cloned()
                .ok_or_else(|| {
                    SandboxStoreError::Corrupt("reused Agent worktree missing".into())
                })?;
            let creation = crate::sandbox::store::AgentCreation {
                agent,
                worktree,
                initial_log,
            };
            creation.validate_replay()?;
            *state = candidate;
            return Ok(AgentAdmissionResult::Existing(creation));
        }
        if matches!(admission.origin, AgentOrigin::WorkspaceProfile { .. })
            && sandbox.status != crate::sandbox::SandboxStatus::Active
        {
            return Err(SandboxStoreError::SandboxNotReady);
        }
        let live = state
            .agents
            .values()
            .filter(|agent| {
                agent.workspace_id == workspace.id
                    && agent.status.is_live()
                    && agent.deleted_at.is_none()
            })
            .count();
        if live >= usize::from(sandbox.limits.max_agents_per_workspace) {
            return Err(SandboxStoreError::AgentLimitReached {
                limit: sandbox.limits.max_agents_per_workspace,
                live: u8::try_from(live).unwrap_or(u8::MAX),
            });
        }

        let creation = admission.materialize_for_parent(&workspace, &sandbox)?;
        let provenance = super::provenance::creation_record(
            &state,
            &admission,
            &creation.agent,
            &sandbox,
            initiation.as_ref(),
        )?;
        if state.worktrees.contains_key(&creation.worktree.id)
            || state.agents.contains_key(&creation.agent.id)
            || state
                .agent_creation_provenance
                .contains_key(&creation.agent.id)
            || state.worktrees.values().any(|stored| {
                stored.workspace_id == workspace.id && stored.name == creation.worktree.name
            })
        {
            return Err(SandboxStoreError::Duplicate);
        }

        let mut candidate = state.clone();
        candidate
            .agent_creation_provenance
            .insert(creation.agent.id.clone(), provenance);
        let mut next_workspace = workspace.clone();
        next_workspace.next_worktree_number = next_workspace
            .next_worktree_number
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        next_workspace.revision = next_workspace
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        candidate
            .workspaces
            .insert(next_workspace.id.clone(), next_workspace);

        let mut next_sandbox = sandbox;
        next_sandbox
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        let cause = match admission.origin {
            AgentOrigin::Manual
            | AgentOrigin::Lifecycle { .. }
            | AgentOrigin::WorkspaceProfile { .. } => SandboxReviveCause::AgentCreation,
            AgentOrigin::Trigger { .. } => SandboxReviveCause::TriggerLaunch,
        };
        let revival = admission.record_parent_admission(
            &mut next_sandbox,
            &workspace,
            cause,
            self.managed_runtime_version().as_deref(),
        )?;
        candidate
            .sandboxes
            .insert(next_sandbox.id.clone(), next_sandbox);

        candidate
            .worktrees
            .insert(creation.worktree.id.clone(), creation.worktree.clone());
        candidate
            .agents
            .insert(creation.agent.id.clone(), creation.agent.clone());
        candidate.agent_logs.insert(
            creation.agent.id.clone(),
            vec![creation.initial_log.clone()],
        );
        if let Some(event) = revival {
            append_sandbox_event(
                &mut candidate,
                &creation.agent.sandbox_id,
                admission.event_at.clone(),
                event,
            )?;
        }
        append_sandbox_event(
            &mut candidate,
            &creation.agent.sandbox_id,
            admission.event_at.clone(),
            SandboxEventKind::WorktreeCreated {
                workspace_id: creation.agent.workspace_id.clone(),
                worktree_id: creation.worktree.id.clone(),
            },
        )?;
        append_sandbox_event(
            &mut candidate,
            &creation.agent.sandbox_id,
            admission.event_at,
            SandboxEventKind::AgentCreated {
                workspace_id: creation.agent.workspace_id.clone(),
                worktree_id: creation.worktree.id.clone(),
                agent_id: creation.agent.id.clone(),
            },
        )?;
        *state = candidate;
        Ok(AgentAdmissionResult::Created(creation))
    }
}

fn replay_agent_admission(
    state: &State,
    admission: &AgentAdmission,
) -> Result<Option<AgentAdmissionResult>, SandboxStoreError> {
    let binding = state
        .agent_reuse_bindings
        .get(&(admission.owner.clone(), admission.creation_key.clone()))
        .cloned();
    if binding
        .as_ref()
        .is_some_and(|(id, _)| !state.agents.contains_key(id))
    {
        return Err(SandboxStoreError::Corrupt(
            "Agent reuse binding target is missing".into(),
        ));
    }
    if binding
        .as_ref()
        .is_some_and(|(_, fingerprint)| admission.fingerprint().ok().as_ref() != Some(fingerprint))
    {
        return Err(SandboxStoreError::AgentCreationConflict);
    }
    if let Some(existing) = state
        .agents
        .values()
        .find(|agent| {
            agent.owner == admission.owner
                && (binding.as_ref().map_or_else(
                    || agent.creation_key == admission.creation_key,
                    |(id, _)| &agent.id == id,
                ))
        })
        .cloned()
    {
        let worktree = state
            .worktrees
            .get(&existing.worktree_id)
            .cloned()
            .ok_or_else(|| SandboxStoreError::Corrupt("agent worktree is missing".into()))?;
        let initial_log = state
            .agent_logs
            .get(&existing.id)
            .and_then(|entries| entries.first())
            .cloned()
            .ok_or_else(|| SandboxStoreError::Corrupt("agent initial log is missing".into()))?;
        let creation = crate::sandbox::store::AgentCreation {
            agent: existing,
            worktree,
            initial_log,
        };
        creation.validate_replay()?;
        return if binding.is_some() || admission.matches(&creation)? {
            Ok(Some(AgentAdmissionResult::Existing(creation)))
        } else {
            Err(SandboxStoreError::AgentCreationConflict)
        };
    }
    Ok(None)
}
