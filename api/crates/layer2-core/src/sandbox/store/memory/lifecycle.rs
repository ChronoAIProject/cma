use super::*;

#[async_trait]
impl SandboxAdmissionStore for InMemorySandboxStore {
    async fn admit_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<SandboxAdmissionResult, SandboxStoreError> {
        admission.validate()?;
        let mut state = self.lock();
        let replay_key = admission
            .trigger_launch_id()
            .map(|launch_id| (admission.sandbox.owner.clone(), launch_id.to_string()));
        if let Some((sandbox_id, fingerprint)) = replay_key
            .as_ref()
            .and_then(|key| state.sandbox_admissions.get(key))
        {
            if fingerprint != &admission.admission_fingerprint {
                return Err(SandboxStoreError::SandboxOriginConflict);
            }
            let sandbox = state.sandboxes.get(sandbox_id).ok_or_else(|| {
                SandboxStoreError::Corrupt("sandbox admission points to a missing aggregate".into())
            })?;
            if sandbox.owner != admission.sandbox.owner
                || sandbox.origin != admission.sandbox.origin
            {
                return Err(SandboxStoreError::Corrupt(
                    "sandbox admission identity disagrees with its aggregate".into(),
                ));
            }
            return Ok(SandboxAdmissionResult::Existing(sandbox.clone()));
        }
        let live = state
            .sandboxes
            .values()
            .filter(|sandbox| {
                sandbox.owner == admission.sandbox.owner && sandbox.retired_at.is_none()
            })
            .count();
        if live >= usize::from(admission.live_limit) {
            return Err(SandboxStoreError::LimitReached {
                limit: admission.live_limit,
                live: u16::try_from(live).unwrap_or(u16::MAX),
            });
        }
        let mut sandbox = admission.sandbox;
        sandbox.name = match admission.name {
            SandboxAdmissionName::Exact(name) => {
                if name_in_use(&state, &sandbox.owner, &name) {
                    return Err(SandboxStoreError::Duplicate);
                }
                name
            }
            SandboxAdmissionName::Allocate(base) => allocated_name(&state, &sandbox.owner, &base),
        };
        let mut candidate = state.clone();
        let id = sandbox.id.clone();
        create_record!(candidate, sandboxes, id.clone(), sandbox.clone())?;
        if let Some(key) = replay_key {
            candidate
                .sandbox_admissions
                .insert(key, (id.clone(), admission.admission_fingerprint));
        }
        append_sandbox_event(
            &mut candidate,
            &id,
            admission.event_at,
            SandboxEventKind::Created,
        )?;
        *state = candidate;
        Ok(SandboxAdmissionResult::Created(sandbox))
    }
}

#[async_trait]
impl SandboxWorkerStore for InMemorySandboxStore {
    async fn due_sandbox_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<SandboxId>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut ids: Vec<_> = self
            .lock()
            .sandboxes
            .values()
            .filter(|sandbox| sandbox.work.is_due(now))
            .map(|sandbox| sandbox.id.clone())
            .collect();
        ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        ids.truncate(usize::from(limit));
        Ok(ids)
    }

    async fn claim_sandbox_work(
        &self,
        id: &SandboxId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Sandbox>, SandboxStoreError> {
        let mut state = self.lock();
        let Some(current) = state.sandboxes.get(id) else {
            return Ok(None);
        };
        if !current.work.is_due(&now) {
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
        claimed.updated_at = now;
        claimed.validate().map_err(|error| {
            SandboxStoreError::Corrupt(format!("claimed sandbox is invalid: {error}"))
        })?;
        state.sandboxes.insert(id.clone(), claimed.clone());
        Ok(Some(claimed))
    }

    async fn purge_sandbox(
        &self,
        id: &SandboxId,
        expected_revision: u64,
        now: &str,
    ) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        let sandbox = state.sandboxes.get(id).ok_or(SandboxStoreError::NotFound)?;
        if sandbox.revision != expected_revision
            || sandbox.status != SandboxStatus::Retired
            || !sandbox.credential.revoke_debt.is_empty()
            || sandbox
                .purge_after
                .as_deref()
                .is_none_or(|deadline| deadline > now)
        {
            return Err(SandboxStoreError::Conflict);
        }
        let agent_ids: Vec<_> = state
            .agents
            .values()
            .filter(|agent| &agent.sandbox_id == id)
            .map(|agent| agent.id.clone())
            .collect();
        state.sandboxes.remove(id);
        state
            .sandbox_admissions
            .retain(|_, (sandbox_id, _)| sandbox_id != id);
        state.sandbox_events.remove(id);
        state.workspaces.retain(|_, row| &row.sandbox_id != id);
        state.worktrees.retain(|_, row| &row.sandbox_id != id);
        state.agents.retain(|_, row| &row.sandbox_id != id);
        for agent_id in agent_ids {
            state.agent_logs.remove(&agent_id);
        }
        Ok(())
    }
}

fn name_in_use(state: &State, owner: &SubjectId, name: &str) -> bool {
    state.sandboxes.values().any(|sandbox| {
        &sandbox.owner == owner && sandbox.name == name && sandbox.retired_at.is_none()
    })
}

fn allocated_name(state: &State, owner: &SubjectId, base: &str) -> String {
    if !name_in_use(state, owner, base) {
        return base.to_string();
    }
    for number in 2_u64.. {
        let suffix = format!("-{number}");
        let maximum = 64_usize.saturating_sub(suffix.len());
        let mut end = base.len().min(maximum);
        while !base.is_char_boundary(end) {
            end -= 1;
        }
        let candidate = format!("{}{}", &base[..end], suffix);
        if !name_in_use(state, owner, &candidate) {
            return candidate;
        }
    }
    unreachable!("the unbounded numeric suffix space always has a free name")
}
