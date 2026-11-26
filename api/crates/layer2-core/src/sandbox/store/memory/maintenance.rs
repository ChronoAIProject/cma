use super::*;

use crate::now_rfc3339;
use crate::sandbox::store::{
    apply_image_rotation, apply_proved_inactivity_pause, validate_image_rotation_request,
    validate_inactivity_pause_request, SandboxImageRotationRequest, SandboxInactivityPauseRequest,
    SandboxMaintenanceApplication, SandboxMaintenanceOutcome, SandboxMaintenanceStore,
};
use crate::sandbox::{Agent, Sandbox, SandboxAgentActivity, SandboxStatus};

#[async_trait]
impl SandboxMaintenanceStore for InMemorySandboxStore {
    async fn admit_custom_runtime_migration(
        &self,
        admission: super::super::SandboxCustomRuntimeMigrationAdmission,
    ) -> Result<Sandbox, SandboxStoreError> {
        super::custom_runtime::admit(self, admission)
    }

    async fn sandbox_maintenance_candidates(
        &self,
        request: &PageRequest,
    ) -> Result<Page<SandboxId>, SandboxStoreError> {
        let values = self
            .lock()
            .sandboxes
            .iter()
            .filter(|(_, sandbox)| {
                matches!(
                    sandbox.status,
                    SandboxStatus::Active | SandboxStatus::Paused
                )
            })
            .map(|(stored_id, sandbox)| {
                if stored_id != &sandbox.id {
                    return Err(SandboxStoreError::Corrupt(
                        "stored Sandbox key disagrees with its identity".into(),
                    ));
                }
                Ok((sandbox.id.to_string(), sandbox.id.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        page(values, request)
    }

    async fn schedule_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        validate_inactivity_pause_request(&request)?;
        let mut state = self.lock();
        let current = state
            .sandboxes
            .get(&request.sandbox_id)
            .cloned()
            .ok_or(SandboxStoreError::NotFound)?;
        let activities = live_agent_activities(&state, &current.id, &request.requested_at)?;
        if inactivity_work_busy(&state, &current, &request.requested_at, &activities)? {
            return Ok(SandboxMaintenanceOutcome::NotEligible);
        }
        let proved_empty = agentless_idle(&state, &current, &request)?;
        if !request.cause.honours_keep_alive() {
            return persist_application(
                &mut state,
                apply_proved_inactivity_pause(&current, &activities, &request, proved_empty)?,
                request.requested_at,
            );
        }
        if current.profile.always_on == Some(true) {
            return Ok(SandboxMaintenanceOutcome::NotEligible);
        }
        let activities = live_agent_activities(&state, &current.id, &request.requested_at)?;
        if request.auto_pause_enabled {
            return persist_application(
                &mut state,
                apply_proved_inactivity_pause(&current, &activities, &request, proved_empty)?,
                request.requested_at,
            );
        }
        if let crate::sandbox::SandboxCredentialSource::Binding { binding_id, .. } =
            &current.credential.source
        {
            let binding = state
                .bindings
                .get(binding_id)
                .ok_or(SandboxStoreError::NotFound)?;
            if binding.sandbox_id != current.id || binding.owner != current.owner {
                return Err(SandboxStoreError::Conflict);
            }
            if !request.auto_pause_enabled
                && binding
                    .profile
                    .environment
                    .as_ref()
                    .is_some_and(|environment| environment.always_keep_alive)
            {
                return Ok(SandboxMaintenanceOutcome::NotEligible);
            }
        }
        self.profiles
            .with_sandbox_availability(&current.profile.id, |availability| {
                if !request.auto_pause_enabled && availability.always_keep_alive {
                    return Ok(SandboxMaintenanceOutcome::NotEligible);
                }
                persist_application(
                    &mut state,
                    apply_proved_inactivity_pause(&current, &activities, &request, proved_empty)?,
                    request.requested_at,
                )
            })
    }

    async fn preview_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        validate_inactivity_pause_request(&request)?;
        let state = self.lock();
        let current = state
            .sandboxes
            .get(&request.sandbox_id)
            .cloned()
            .ok_or(SandboxStoreError::NotFound)?;
        let activities = live_agent_activities(&state, &current.id, &request.requested_at)?;
        if inactivity_work_busy(&state, &current, &request.requested_at, &activities)? {
            return Ok(SandboxMaintenanceOutcome::NotEligible);
        }
        let proved_empty = agentless_idle(&state, &current, &request)?;
        if !request.cause.honours_keep_alive() {
            return preview_application(&current, &activities, &request, proved_empty);
        }
        if current.profile.always_on == Some(true) {
            return Ok(SandboxMaintenanceOutcome::NotEligible);
        }
        let activities = live_agent_activities(&state, &current.id, &request.requested_at)?;
        if request.auto_pause_enabled {
            return preview_application(&current, &activities, &request, proved_empty);
        }
        if let crate::sandbox::SandboxCredentialSource::Binding { binding_id, .. } =
            &current.credential.source
        {
            let binding = state
                .bindings
                .get(binding_id)
                .ok_or(SandboxStoreError::NotFound)?;
            if binding.sandbox_id != current.id || binding.owner != current.owner {
                return Err(SandboxStoreError::Conflict);
            }
            if binding
                .profile
                .environment
                .as_ref()
                .is_some_and(|environment| environment.always_keep_alive)
            {
                return Ok(SandboxMaintenanceOutcome::NotEligible);
            }
        }
        self.profiles
            .with_sandbox_availability(&current.profile.id, |availability| {
                if availability.always_keep_alive {
                    return Ok(SandboxMaintenanceOutcome::NotEligible);
                }
                preview_application(&current, &activities, &request, proved_empty)
            })
    }

    async fn set_sandbox_always_on(
        &self,
        sandbox_id: SandboxId,
        expected_revision: u64,
        always_on: bool,
    ) -> Result<Sandbox, SandboxStoreError> {
        let mut state = self.lock();
        let current = state
            .sandboxes
            .get(&sandbox_id)
            .cloned()
            .ok_or(SandboxStoreError::NotFound)?;
        if matches!(
            current.status,
            SandboxStatus::Retiring | SandboxStatus::Retired
        ) || current.revision != expected_revision
        {
            return Err(SandboxStoreError::Conflict);
        }
        let mut next = current.clone();
        next.profile.always_on = Some(always_on);
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        next.updated_at = now_rfc3339();
        next.validate()
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        state.sandboxes.insert(sandbox_id, next.clone());
        Ok(next)
    }

    async fn schedule_image_rotation(
        &self,
        request: SandboxImageRotationRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        validate_image_rotation_request(&request)?;
        let mut state = self.lock();
        let current = state
            .sandboxes
            .get(&request.sandbox_id)
            .cloned()
            .ok_or(SandboxStoreError::NotFound)?;
        let agents = sandbox_agents(&state, &current.id)?;
        if state.workspaces.values().any(|workspace| {
            workspace.sandbox_id == current.id
                && (workspace.work.step.is_pending() || workspace.work.lease.is_some())
        }) {
            return Ok(SandboxMaintenanceOutcome::NotEligible);
        }
        persist_application(
            &mut state,
            apply_image_rotation(&current, &agents, &request)?,
            request.requested_at,
        )
    }
}

/// Evidence every automatic pause needs beyond the live Agents: no
/// Workspace has pending or leased work, and a Sandbox without live Agents has
/// no open owner session.
fn inactivity_work_busy(
    state: &State,
    sandbox: &Sandbox,
    observed_at: &str,
    activities: &[SandboxAgentActivity],
) -> Result<bool, SandboxStoreError> {
    if state
        .sandbox_controls
        .values()
        .any(|record| record.envelope.sandbox_id == sandbox.id && record.queued())
    {
        return Ok(true);
    }
    let workspace_busy = state.workspaces.values().any(|workspace| {
        workspace.sandbox_id == sandbox.id
            && (workspace.work.step.is_pending() || workspace.work.lease.is_some())
    });
    if workspace_busy || !activities.is_empty() {
        return Ok(workspace_busy);
    }
    let observed = crate::seconds_from_rfc3339(observed_at).ok_or(
        SandboxStoreError::InvalidTransaction("sandbox session observation is invalid"),
    )?;
    Ok(state.sandbox_sessions.iter().any(|((id, _), expires_at)| {
        id == &sandbox.id
            && crate::seconds_from_rfc3339(expires_at).is_some_and(|expires| expires > observed)
    }) || crate::sandbox::has_open_desktop_session(&sandbox.id))
}

pub(super) fn live_agent_activities(
    state: &State,
    sandbox_id: &SandboxId,
    observed_at: &str,
) -> Result<Vec<SandboxAgentActivity>, SandboxStoreError> {
    let observed = crate::seconds_from_rfc3339(observed_at).ok_or(
        SandboxStoreError::InvalidTransaction("sandbox session observation is invalid"),
    )?;
    let open_session = state.sandbox_sessions.iter().any(|((id, _), expires_at)| {
        id == sandbox_id
            && crate::seconds_from_rfc3339(expires_at).is_some_and(|expires| expires > observed)
    });
    sandbox_agents(state, sandbox_id)?
        .into_iter()
        .filter(|agent| agent.status.is_live())
        .map(|agent| {
            let newest = newest_agent_log(state, &agent)?;
            let mut activity =
                SandboxAgentActivity::from_live_agent(&agent, newest).map_err(|error| {
                    SandboxStoreError::Corrupt(format!("stored live Agent is invalid: {error}"))
                })?;
            // A command can be admitted before its Agent checkpoint catches up.
            // Treat every nonterminal durable receipt as external activity so a
            // queued UI/API/channel/trigger input cannot be mistaken for idle.
            activity.external_activity_present = state.agent_commands.values().any(|command| {
                command.receipt.envelope.agent_id == agent.id && !command.status.is_terminal()
            });
            activity.open_terminal_or_desktop_session =
                open_session || crate::sandbox::has_open_desktop_session(&agent.sandbox_id);
            Ok(activity)
        })
        .collect()
}

pub(super) fn sandbox_agents(
    state: &State,
    sandbox_id: &SandboxId,
) -> Result<Vec<Agent>, SandboxStoreError> {
    state
        .agents
        .iter()
        .filter(|(_, agent)| &agent.sandbox_id == sandbox_id)
        .map(|(stored_id, agent)| {
            if stored_id != &agent.id {
                return Err(SandboxStoreError::Corrupt(
                    "stored Agent key disagrees with its identity".into(),
                ));
            }
            Ok(agent.clone())
        })
        .collect()
}

fn newest_agent_log<'a>(
    state: &'a State,
    agent: &Agent,
) -> Result<Option<&'a AgentLogEntry>, SandboxStoreError> {
    let Some(entries) = state.agent_logs.get(&agent.id) else {
        return Ok(None);
    };
    for (index, entry) in entries.iter().enumerate() {
        let expected = u64::try_from(index)
            .map_err(|_| SandboxStoreError::SequenceConflict)?
            .checked_add(1)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        if entry.seq != expected {
            return Err(SandboxStoreError::SequenceConflict);
        }
        if entry.agent_id != agent.id {
            return Err(SandboxStoreError::Corrupt(
                "stored Agent log disagrees with its Agent".into(),
            ));
        }
    }
    Ok(entries.last())
}

pub(super) fn persist_application(
    state: &mut State,
    application: SandboxMaintenanceApplication,
    appended_at: String,
) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
    let (expected_revision, sandbox, event) = match application {
        SandboxMaintenanceApplication::Persist {
            expected_revision,
            sandbox,
            event,
        } => (expected_revision, sandbox, event),
        SandboxMaintenanceApplication::Complete(outcome) => return Ok(outcome),
    };

    validate_event_stream(state, &sandbox.id)?;
    let mut candidate = state.clone();
    let id = sandbox.id.clone();
    save_record!(
        candidate,
        sandboxes,
        id.clone(),
        expected_revision,
        *sandbox
    )?;
    append_sandbox_event(&mut candidate, &id, appended_at, event)?;
    *state = candidate;
    Ok(SandboxMaintenanceOutcome::Scheduled)
}

fn preview_application(
    current: &Sandbox,
    activities: &[SandboxAgentActivity],
    request: &SandboxInactivityPauseRequest,
    proved_empty: bool,
) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
    match apply_proved_inactivity_pause(current, activities, request, proved_empty)? {
        SandboxMaintenanceApplication::Complete(outcome) => Ok(outcome),
        SandboxMaintenanceApplication::Persist { .. } => Ok(SandboxMaintenanceOutcome::Scheduled),
    }
}

pub(super) fn validate_event_stream(
    state: &State,
    sandbox_id: &SandboxId,
) -> Result<(), SandboxStoreError> {
    for (index, event) in state
        .sandbox_events
        .get(sandbox_id)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let expected = u64::try_from(index)
            .map_err(|_| SandboxStoreError::SequenceConflict)?
            .checked_add(1)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        if event.seq != expected {
            return Err(SandboxStoreError::SequenceConflict);
        }
        if &event.sandbox_id != sandbox_id {
            return Err(SandboxStoreError::Corrupt(
                "stored Sandbox event disagrees with its Sandbox".into(),
            ));
        }
    }
    Ok(())
}

fn agentless_idle(
    state: &State,
    sandbox: &Sandbox,
    request: &SandboxInactivityPauseRequest,
) -> Result<bool, SandboxStoreError> {
    let agents = sandbox_agents(state, &sandbox.id)?;
    let workspaces: Vec<_> = state
        .workspaces
        .values()
        .filter(|workspace| workspace.sandbox_id == sandbox.id)
        .cloned()
        .collect();
    Ok(crate::sandbox::agentless_idle_with_children(
        sandbox,
        &agents,
        &workspaces,
        &request.requested_at,
        request.pause_after_seconds,
    ) && !state.agent_commands.values().any(|command| {
        agents
            .iter()
            .any(|agent| agent.id == command.receipt.envelope.agent_id)
            && !command.status.is_terminal()
    }) && !state
        .sandbox_controls
        .values()
        .any(|command| command.envelope.sandbox_id == sandbox.id && command.queued()))
}
