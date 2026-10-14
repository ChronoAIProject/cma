use super::*;
use crate::sandbox::{SandboxReviveCause, WorkspaceOrigin, WorkspaceStatus, WorkspaceWorkStep};

#[async_trait]
impl WorkspaceAdmissionStore for InMemorySandboxStore {
    async fn admit_workspace(
        &self,
        admission: WorkspaceAdmission,
    ) -> Result<WorkspaceAdmissionResult, SandboxStoreError> {
        let mut state = self.lock();
        if let Some(fence) = &admission.binding {
            let binding = state
                .bindings
                .get(&fence.binding_id)
                .ok_or(SandboxStoreError::Conflict)?;
            fence.require_workspace(binding, &admission.workspace)?;
        }
        let workspace = admission.workspace;
        let sandbox = state
            .sandboxes
            .get(&workspace.sandbox_id)
            .ok_or(SandboxStoreError::SandboxNotReady)?;
        if !admission.kind.accepts(sandbox) {
            return Err(SandboxStoreError::SandboxNotReady);
        }
        workspace
            .validate_for_admission(sandbox, sandbox.status)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        validate_fresh(&workspace, admission.kind)?;
        if !workspace.origin_matches_sandbox_profile(sandbox) {
            return Err(SandboxStoreError::WorkspaceOriginConflict);
        }

        if admission.kind != WorkspaceAdmissionKind::Manual {
            if let Some(existing) = state.workspaces.values().find(|stored| {
                stored.sandbox_id == workspace.sandbox_id && stored.origin == workspace.origin
            }) {
                if same_profile_origin(existing, &workspace) {
                    return Ok(WorkspaceAdmissionResult::Existing(existing.clone()));
                }
                return Err(SandboxStoreError::WorkspaceOriginConflict);
            }
        }
        if state.workspaces.contains_key(&workspace.id) {
            return Err(SandboxStoreError::Duplicate);
        }
        if state.workspaces.values().any(|stored| {
            stored.sandbox_id == workspace.sandbox_id
                && stored.name == workspace.name
                && stored.removed_at.is_none()
        }) {
            return Err(SandboxStoreError::WorkspaceNameConflict);
        }
        let live = state
            .workspaces
            .values()
            .filter(|stored| {
                stored.sandbox_id == workspace.sandbox_id && stored.removed_at.is_none()
            })
            .count();
        if live >= usize::from(sandbox.limits.max_workspaces) {
            return Err(SandboxStoreError::WorkspaceLimitReached {
                limit: sandbox.limits.max_workspaces,
                live: u8::try_from(live).unwrap_or(u8::MAX),
            });
        }

        let mut candidate = state.clone();
        if admission.kind != WorkspaceAdmissionKind::SandboxProfile {
            let mut next_sandbox = sandbox.clone();
            let cause = match admission.kind {
                WorkspaceAdmissionKind::Trigger => SandboxReviveCause::TriggerLaunch,
                _ => SandboxReviveCause::WorkspaceCreation,
            };
            let event = next_sandbox
                .record_creation_admission(cause, admission.event_at.clone())
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            candidate
                .sandboxes
                .insert(next_sandbox.id.clone(), next_sandbox);
            if let Some(event) = event {
                append_sandbox_event(
                    &mut candidate,
                    &workspace.sandbox_id,
                    admission.event_at.clone(),
                    event,
                )?;
            }
        }
        let id = workspace.id.clone();
        let sandbox_id = workspace.sandbox_id.clone();
        create_record!(candidate, workspaces, id.clone(), workspace.clone())?;
        append_sandbox_event(
            &mut candidate,
            &sandbox_id,
            admission.event_at,
            SandboxEventKind::WorkspaceCreated { workspace_id: id },
        )?;
        *state = candidate;
        Ok(WorkspaceAdmissionResult::Created(workspace))
    }

    async fn begin_workspace_removal(
        &self,
        id: &WorkspaceId,
        owner: &SubjectId,
        now: String,
    ) -> Result<Workspace, SandboxStoreError> {
        let mut state = self.lock();
        let current = state
            .workspaces
            .get(id)
            .filter(|workspace| &workspace.owner == owner)
            .cloned()
            .ok_or(SandboxStoreError::NotFound)?;
        if matches!(
            current.status,
            WorkspaceStatus::Removing | WorkspaceStatus::Removed
        ) {
            return Ok(current);
        }
        let parent = state
            .sandboxes
            .get(&current.sandbox_id)
            .ok_or(SandboxStoreError::NotFound)?;
        if parent.lifecycle.checkpoint_capture.is_some() {
            return Err(SandboxStoreError::Conflict);
        }
        if state.agents.values().any(|agent| {
            agent.workspace_id == current.id
                && (agent.owner != current.owner || agent.sandbox_id != current.sandbox_id)
        }) {
            return Err(SandboxStoreError::Corrupt(
                "workspace child lineage is inconsistent".into(),
            ));
        }
        let mut next = current;
        next.transition(WorkspaceStatus::Removing, now.clone(), None)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        next.work.schedule(WorkspaceWorkStep::Remove, now);
        state.workspaces.insert(id.clone(), next.clone());
        Ok(next)
    }
}

#[async_trait]
impl WorkspaceWorkerStore for InMemorySandboxStore {
    async fn due_workspace_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<WorkspaceId>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut ids: Vec<_> = self
            .lock()
            .workspaces
            .values()
            .filter(|workspace| workspace.work.is_due(now))
            .map(|workspace| workspace.id.clone())
            .collect();
        ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        ids.truncate(usize::from(limit));
        Ok(ids)
    }

    async fn claim_workspace_work(
        &self,
        id: &WorkspaceId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Workspace>, SandboxStoreError> {
        let mut state = self.lock();
        let Some(current) = state.workspaces.get(id) else {
            return Ok(None);
        };
        let parent = state
            .sandboxes
            .get(&current.sandbox_id)
            .ok_or(SandboxStoreError::NotFound)?;
        if !current.work.is_due(&now) || parent.lifecycle.checkpoint_capture.is_some() {
            return Ok(None);
        }
        let mut claimed = current.clone();
        claimed
            .work
            .claim(holder.to_string(), &now, lease_expires_at)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        claimed.revision = claimed
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        state.workspaces.insert(id.clone(), claimed.clone());
        Ok(Some(claimed))
    }
}

fn validate_fresh(
    workspace: &Workspace,
    kind: WorkspaceAdmissionKind,
) -> Result<(), SandboxStoreError> {
    let origin_matches = matches!(
        (&workspace.origin, kind),
        (WorkspaceOrigin::Manual, WorkspaceAdmissionKind::Manual)
            | (
                WorkspaceOrigin::Lifecycle { .. },
                WorkspaceAdmissionKind::Lifecycle
            )
            | (
                WorkspaceOrigin::SandboxProfile { .. },
                WorkspaceAdmissionKind::SandboxProfile
            )
            | (
                WorkspaceOrigin::Trigger { .. },
                WorkspaceAdmissionKind::Trigger
            )
    );
    if !origin_matches
        || workspace.revision != 1
        || workspace.status != WorkspaceStatus::Preparing
        || workspace.work.step != WorkspaceWorkStep::Prepare
        || workspace.resolved_commit.is_some()
        || workspace.ready_at.is_some()
        || workspace.failed.is_some()
        || workspace.removed_at.is_some()
    {
        return Err(SandboxStoreError::InvalidTransaction(
            "workspace admission is not a fresh preparing aggregate",
        ));
    }
    Ok(())
}

fn same_profile_origin(existing: &Workspace, candidate: &Workspace) -> bool {
    existing.owner == candidate.owner
        && existing.profile.id == candidate.profile.id
        && existing.profile.revision == candidate.profile.revision
        && existing.profile.digest == candidate.profile.digest
}
