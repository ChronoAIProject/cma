use super::*;

#[async_trait]
impl SandboxAdmissionStore for InMemorySandboxStore {
    async fn sandbox_admission_replay(
        &self,
        owner: &SubjectId,
        origin: &SandboxOrigin,
    ) -> Result<Option<SandboxAdmissionReplay>, SandboxStoreError> {
        sandbox_admission_replay(&self.lock(), owner, origin)
    }

    async fn admit_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<SandboxAdmissionResult, SandboxStoreError> {
        admission.validate()?;
        let mut state = self.lock();
        let mut resource_policy = None;
        if let Some(fence) = &admission.binding {
            let binding = state
                .bindings
                .get(&fence.binding_id)
                .ok_or(SandboxStoreError::Conflict)?;
            fence.require_sandbox(binding, &admission.sandbox)?;
            resource_policy = binding.sandbox_resource_policy(&admission.sandbox)?;
        }
        let replay_key = admission.origin_key().map(|(kind, id)| {
            (
                admission.sandbox.owner.clone(),
                kind.to_owned(),
                id.to_owned(),
            )
        });
        if let Some(replay) =
            sandbox_admission_replay(&state, &admission.sandbox.owner, &admission.sandbox.origin)?
        {
            if replay.admission_fingerprint != admission.admission_fingerprint {
                return Err(SandboxStoreError::SandboxOriginConflict);
            }
            if replay.sandbox.owner != admission.sandbox.owner
                || replay.sandbox.origin != admission.sandbox.origin
            {
                return Err(SandboxStoreError::Corrupt(
                    "sandbox admission identity disagrees with its aggregate".into(),
                ));
            }
            return Ok(SandboxAdmissionResult::Existing(replay.sandbox));
        }
        super::admission::require_operator_open(&state, admission.admission_cause())?;
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
        let admission_name = admission.name;
        sandbox.name = match &admission_name {
            SandboxAdmissionName::Exact(name) => {
                if name_in_use(&state, &sandbox.owner, name) {
                    return Err(SandboxStoreError::Duplicate);
                }
                name.clone()
            }
            SandboxAdmissionName::Allocate(base) => allocated_name(&state, &sandbox.owner, base),
        };
        let mut candidate = state.clone();
        let id = sandbox.id.clone();
        create_record!(candidate, sandboxes, id.clone(), sandbox.clone())?;
        if let Some(policy) = resource_policy {
            candidate.resource_policies.insert(id.clone(), policy);
        }
        if let Some(key) = replay_key {
            candidate.sandbox_admissions.insert(
                key,
                (id.clone(), admission_name, admission.admission_fingerprint),
            );
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
        if !current.work.is_due(&now)
            || current.lifecycle.checkpoint_capture.is_some()
            || current.retirement_archive_pending()
        {
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

    async fn renew_sandbox_authority(
        &self,
        claimed: &Sandbox,
        now: &str,
        requested_until: &str,
    ) -> Result<Option<super::super::WorkLeaseGrant>, SandboxStoreError> {
        self.renew_sandbox_work(claimed, now, requested_until)
            .await?
            .map(|until| super::super::WorkLeaseGrant::at(until, now))
            .transpose()
    }

    async fn renew_sandbox_work(
        &self,
        claimed: &Sandbox,
        now: &str,
        requested_until: &str,
    ) -> Result<Option<String>, SandboxStoreError> {
        let Some(original) = &claimed.work.lease else {
            return Err(SandboxStoreError::Conflict);
        };
        let until = super::super::work_lease::renewal_deadline(now, requested_until)?;
        let mut state = self.lock();
        let Some(current) = state.sandboxes.get_mut(&claimed.id) else {
            return Ok(None);
        };
        if current.work.step != claimed.work.step
            || current.owner != claimed.owner
            || current
                .lifecycle
                .operation
                .as_ref()
                .map(|value| &value.authority().operation_id)
                != claimed
                    .lifecycle
                    .operation
                    .as_ref()
                    .map(|value| &value.authority().operation_id)
            || current.retirement_archive_pending()
            || current.lifecycle.checkpoint_capture.is_some()
        {
            return Ok(None);
        }
        let Some(lease) = current.work.lease.as_mut().filter(|lease| {
            lease.holder == original.holder
                && lease.fence == original.fence
                && lease.expires_at.as_str() > now
        }) else {
            return Ok(None);
        };
        lease.expires_at = lease.expires_at.clone().max(until);
        Ok(Some(lease.expires_at.clone()))
    }

    async fn purge_sandbox(
        &self,
        id: &SandboxId,
        expected_revision: u64,
        now: &str,
    ) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        let sandbox = state.sandboxes.get(id).ok_or(SandboxStoreError::NotFound)?;
        if sandbox.retirement_archive_pending()
            || state
                .agents
                .values()
                .any(|agent| &agent.sandbox_id == id && agent.conversation.turn_timeout.is_some())
            || sandbox.revision != expected_revision
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
            .retain(|_, (sandbox_id, _, _)| sandbox_id != id);
        state.sandbox_events.remove(id);
        state.workspaces.retain(|_, row| &row.sandbox_id != id);
        state.worktrees.retain(|_, row| &row.sandbox_id != id);
        state.agents.retain(|_, row| &row.sandbox_id != id);
        state
            .agent_reuse_bindings
            .retain(|_, (agent_id, _)| !agent_ids.contains(agent_id));
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
    for number in 1_u64.. {
        let candidate = allocated_sandbox_name(base, number)
            .expect("a validated admission name can always be allocated");
        if !name_in_use(state, owner, &candidate) {
            return candidate;
        }
    }
    unreachable!("the unbounded numeric suffix space always has a free name")
}

fn sandbox_admission_replay(
    state: &State,
    owner: &SubjectId,
    origin: &SandboxOrigin,
) -> Result<Option<SandboxAdmissionReplay>, SandboxStoreError> {
    let Some((kind, id)) = origin.replay_key() else {
        return Ok(None);
    };
    let matching_ids = state
        .sandboxes
        .values()
        .filter(|sandbox| sandbox.owner == *owner && sandbox.origin == *origin)
        .map(|sandbox| &sandbox.id)
        .collect::<Vec<_>>();
    let Some((sandbox_id, name, fingerprint)) =
        state
            .sandbox_admissions
            .get(&(owner.clone(), kind.to_owned(), id.to_owned()))
    else {
        return if matching_ids.is_empty() {
            Ok(None)
        } else {
            Err(SandboxStoreError::Corrupt(
                "sandbox is missing admission replay metadata".into(),
            ))
        };
    };
    let sandbox = state.sandboxes.get(sandbox_id).ok_or_else(|| {
        SandboxStoreError::Corrupt("sandbox admission points to a missing aggregate".into())
    })?;
    if matching_ids.as_slice() != [sandbox_id]
        || sandbox.owner != *owner
        || sandbox.origin != *origin
    {
        return Err(SandboxStoreError::Corrupt(
            "sandbox admission origin disagrees with its aggregate".into(),
        ));
    }
    SandboxAdmissionReplay::from_retained(sandbox.clone(), name.clone(), fingerprint.clone())
        .map(Some)
}
