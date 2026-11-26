use super::*;
use crate::ids::SandboxCommandId;
use crate::sandbox::control::*;
use std::sync::Arc;

#[async_trait]
impl SandboxControlStore for InMemorySandboxStore {
    async fn reserve_sandbox_control(
        &self,
        record: SandboxControlRecord,
        authorization: Arc<SandboxControlAuthorization>,
        new_commitment: Result<(), crate::admission::AdmissionRefusal>,
    ) -> Result<SandboxControlRecord, SandboxControlError> {
        record.validate()?;
        if !record.queued() || authorization.envelope() != &record.envelope {
            return Err(SandboxControlError::Invalid);
        }
        if record.admission_audit
            != authorization
                .audit()
                .map_err(|_| SandboxControlError::Conflict)?
        {
            return Err(SandboxControlError::Conflict);
        }
        let mut state = self.lock();
        let snapshot = super::resource_access::snapshot(&state, &record.envelope.target())
            .map_err(|_| SandboxControlError::NotFound)?;
        authorization
            .validate_snapshot(&snapshot)
            .map_err(|_| SandboxControlError::Conflict)?;
        let key = (record.envelope.scope(), record.key_sha256.clone());
        if let Some(id) = state.sandbox_control_keys.get(&key) {
            let old = state
                .sandbox_controls
                .get(id)
                .ok_or(SandboxControlError::Corrupt)?;
            return record.replay(old.clone());
        }
        new_commitment.map_err(SandboxControlError::from)?;
        if record.envelope.expected_revision != snapshot.lineage.sandbox.revision
            || record.expired(&crate::now_rfc3339())?
        {
            return Err(SandboxControlError::Conflict);
        }
        if state
            .sandbox_controls
            .values()
            .any(|old| old.envelope.sandbox_id == record.envelope.sandbox_id && old.queued())
        {
            return Err(SandboxControlError::Busy);
        }
        if state
            .sandbox_controls
            .contains_key(&record.envelope.command_id)
        {
            return Err(SandboxControlError::Conflict);
        }
        let id = record.envelope.command_id.clone();
        authorization
            .require_current()
            .map_err(|_| SandboxControlError::Conflict)?;
        state.sandbox_control_keys.insert(key, id.clone());
        state.sandbox_controls.insert(id, record.clone());
        Ok(record)
    }

    async fn sandbox_control(
        &self,
        id: &SandboxCommandId,
    ) -> Result<Option<SandboxControlRecord>, SandboxControlError> {
        let value = self.lock().sandbox_controls.get(id).cloned();
        if let Some(value) = &value {
            value.validate()?;
        }
        Ok(value)
    }

    async fn queued_sandbox_controls(
        &self,
        page: &PageRequest,
    ) -> Result<Page<SandboxControlRecord>, SandboxControlError> {
        PageRequest::new(page.limit, page.cursor.clone())
            .map_err(|_| SandboxControlError::Invalid)?;
        let state = self.lock();
        let mut rows: Vec<_> = state
            .sandbox_controls
            .values()
            .filter(|record| {
                record.queued()
                    && page
                        .cursor
                        .as_deref()
                        .is_none_or(|cursor| record.envelope.command_id.as_str() > cursor)
            })
            .cloned()
            .collect();
        rows.sort_by(|left, right| left.envelope.command_id.cmp(&right.envelope.command_id));
        rows.truncate(usize::from(page.limit) + 1);
        for row in &rows {
            row.validate()?;
        }
        let more = rows.len() > usize::from(page.limit);
        rows.truncate(usize::from(page.limit));
        let next = more.then(|| {
            rows.last()
                .expect("positive page")
                .envelope
                .command_id
                .to_string()
        });
        Ok(Page { items: rows, next })
    }

    async fn apply_sandbox_control(
        &self,
        record: &SandboxControlRecord,
        authorization: Arc<SandboxControlAuthorization>,
    ) -> Result<SandboxControlRecord, SandboxControlError> {
        record.validate()?;
        if authorization.envelope() != &record.envelope {
            return Err(SandboxControlError::Conflict);
        }
        let mut state = self.lock();
        require_record(&state, record)?;
        let snapshot = super::resource_access::snapshot(&state, &record.envelope.target())
            .map_err(|_| SandboxControlError::NotFound)?;
        authorization
            .validate_snapshot(&snapshot)
            .map_err(|_| SandboxControlError::Conflict)?;
        let timestamp = crate::now_rfc3339();
        let at = timestamp.as_str();
        let plan = plan_sandbox_control(
            record,
            &snapshot.lineage.sandbox,
            at,
            self.managed_runtime_version().as_deref(),
        )?;
        let mut candidate = state.clone();
        let mut next = record.clone();
        next.revision = 2;
        match plan {
            SandboxControlPlan::Reject(reason) => next = record.rejected(reason, at)?,
            SandboxControlPlan::NoChange { disposition } => {
                next.status = SandboxControlStatus::Applied {
                    at: at.into(),
                    sandbox_revision: snapshot.lineage.sandbox.revision,
                    disposition,
                };
                next.execution_audit = Some(
                    authorization
                        .audit()
                        .map_err(|_| SandboxControlError::Conflict)?,
                );
            }
            SandboxControlPlan::Transition { sandbox, event } => {
                next.status = SandboxControlStatus::Applied {
                    at: at.into(),
                    sandbox_revision: sandbox.revision,
                    disposition: SandboxControlDisposition::Scheduled,
                };
                next.execution_audit = Some(
                    authorization
                        .audit()
                        .map_err(|_| SandboxControlError::Conflict)?,
                );
                candidate.sandboxes.insert(sandbox.id.clone(), *sandbox);
                append_sandbox_event(
                    &mut candidate,
                    &record.envelope.sandbox_id,
                    at.into(),
                    event,
                )
                .map_err(|_| SandboxControlError::Corrupt)?;
            }
        }
        next.validate()?;
        authorization
            .require_current()
            .map_err(|_| SandboxControlError::Conflict)?;
        candidate
            .sandbox_controls
            .insert(record.envelope.command_id.clone(), next.clone());
        *state = candidate;
        Ok(next)
    }

    async fn reject_sandbox_control(
        &self,
        record: &SandboxControlRecord,
        reason: SandboxControlFailure,
    ) -> Result<SandboxControlRecord, SandboxControlError> {
        let next = record.rejected(reason, &crate::now_rfc3339())?;
        let mut state = self.lock();
        require_record(&state, record)?;
        state
            .sandbox_controls
            .insert(record.envelope.command_id.clone(), next.clone());
        Ok(next)
    }
}

fn require_record(
    state: &State,
    expected: &SandboxControlRecord,
) -> Result<(), SandboxControlError> {
    expected.validate()?;
    if !expected.queued()
        || state.sandbox_controls.get(&expected.envelope.command_id) != Some(expected)
    {
        return Err(SandboxControlError::Conflict);
    }
    Ok(())
}
