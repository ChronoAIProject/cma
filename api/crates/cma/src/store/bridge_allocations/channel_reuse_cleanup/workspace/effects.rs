use super::*;
use crate::sandbox::owned_workspace_cleanup::WorkspaceCleanupState;
use layer3_adapter::{
    WorkspaceRemovalObservation, WorkspaceRemovalRequest, WorkspaceRemovalSupport,
};

impl MongoBridgeAllocationStore {
    pub(crate) async fn workspace_cleanup_holds(
        &self,
        permit: &WorkspaceCleanupAuthority,
    ) -> Result<Vec<WorkspaceCleanupHold>, CleanupError> {
        let mut session = self
            .bridges
            .client
            .start_session()
            .await
            .map_err(|_| CleanupError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| CleanupError::Unavailable)?;
        let result = async {
            let (_, state, _) = proof::claimed(self, permit, &mut session).await?;
            let database = self.bridges.client.database(&self.records.namespace().db);
            let (_, holds) = ownership::require(
                &database,
                permit.common().allocation(),
                &permit.common().parent().lineage.sandbox,
                &state.child,
                state.progress == WorkspaceCleanupProgress::Complete,
                &mut session,
            )
            .await?;
            permit.require_current()?;
            Ok::<_, JobError>(holds)
        }
        .await;
        let result = match result {
            Ok(value) => value,
            Err(error) => {
                let _ = session.abort_transaction().await;
                return Err(error.core());
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(result),
            _ => Err(CleanupError::Unavailable),
        }
    }
    pub(crate) async fn step_workspace_cleanup(
        &self,
        permit: &WorkspaceCleanupAuthority,
        support: &WorkspaceRemovalSupport,
        completed: Option<&WorkspaceRemovalObservation>,
    ) -> Result<WorkspaceCleanupState, CleanupError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1 << (attempt - 1).min(5)))
                    .await;
            }
            permit
                .require_current()
                .map_err(|_| CleanupError::Unavailable)?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| CleanupError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| CleanupError::Unavailable)?;
            let result = self
                .workspace_effect_transaction(permit, support, completed, &mut session)
                .await;
            let result = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if error.retryable() {
                        continue;
                    }
                    return Err(error.core());
                }
            };
            if permit.require_current().is_err() {
                let _ = session.abort_transaction().await;
                return Err(CleanupError::Unavailable);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                _ => return Err(CleanupError::Unavailable),
            }
        }
        Err(CleanupError::Unavailable)
    }
    async fn workspace_effect_transaction(
        &self,
        permit: &WorkspaceCleanupAuthority,
        support: &WorkspaceRemovalSupport,
        completed: Option<&WorkspaceRemovalObservation>,
        session: &mut ClientSession,
    ) -> Result<WorkspaceCleanupState, JobError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let (_, state, workspace) = proof::claimed(self, permit, session).await?;
        let sandbox = &permit.common().parent().lineage.sandbox;
        admission::require_support(sandbox, support)?;
        if state.progress >= WorkspaceCleanupProgress::RemovalStarted
            && (state.runtime.as_ref() != sandbox.runtime.as_ref()
                || state.removal.as_ref().is_none_or(|request| {
                    !layer3_adapter::same_runtime_writer(&request.runtime, &support.runtime)
                }))
        {
            return Err(CleanupError::Conflict.into());
        }
        if completed.is_some() && state.progress != WorkspaceCleanupProgress::RemovalStarted {
            return Err(CleanupError::Conflict.into());
        }
        let mut next = match state.progress {
            WorkspaceCleanupProgress::Requested => {
                let (_, holds) = ownership::require(
                    &database,
                    permit.common().allocation(),
                    sandbox,
                    &state.child,
                    false,
                    session,
                )
                .await?;
                if !holds.is_empty() {
                    return Err(CleanupError::Conflict.into());
                }
                let row = read(&database, "workspaces", workspace.id.as_str(), session).await?;
                if row.contains_key(WORKSPACE_ADMISSION_FIELD)
                    || row.contains_key(CLEANUP_EXECUTION_FIELD)
                {
                    return Err(CleanupError::Conflict.into());
                }
                let changed = majority_collection::<Document>(&database, "workspaces").update_one(row,
                    doc! {"$set":{WORKSPACE_ADMISSION_FIELD:records::marker(&state.admission)},"$inc":{"owned_workspace_cleanup_fence":1_i64}})
                    .session(&mut *session).await?;
                if changed.matched_count != 1 {
                    return Err(CleanupError::Conflict.into());
                }
                state.advance(WorkspaceCleanupProgress::AdmissionClosed)?
            }
            WorkspaceCleanupProgress::AdmissionClosed => {
                let mut next = state.advance(WorkspaceCleanupProgress::RemovalStarted)?;
                next.runtime = sandbox.runtime.clone();
                next.removal = Some(WorkspaceRemovalRequest {
                    runtime: support.runtime.clone(),
                    request: state.canonical_request(),
                });
                for (collection, id) in [
                    ("sandboxes", sandbox.id.as_str()),
                    ("workspaces", workspace.id.as_str()),
                ] {
                    let row = read(&database, collection, id, session).await?;
                    if row.contains_key(CLEANUP_EXECUTION_FIELD) {
                        return Err(CleanupError::Conflict.into());
                    }
                    let changed = majority_collection::<Document>(&database, collection).update_one(row,
                        doc! {"$set":{CLEANUP_EXECUTION_FIELD:state.id()},"$inc":{"owned_workspace_cleanup_fence":1_i64}})
                        .session(&mut *session).await?;
                    if changed.matched_count != 1 {
                        return Err(CleanupError::Conflict.into());
                    }
                }
                next
            }
            WorkspaceCleanupProgress::RemovalStarted => {
                let Some(completed) = completed else {
                    return Ok(state);
                };
                completed
                    .validate_for(state.removal.as_ref().ok_or(CleanupError::Corrupt)?)
                    .map_err(|_| CleanupError::Corrupt)?;
                if completed.outcome != layer3_adapter::WorkspaceRemovalOutcome::CurrentlyAbsent {
                    return Err(CleanupError::Conflict.into());
                }
                let mut next = state.advance(WorkspaceCleanupProgress::RuntimeRemoved)?;
                next.observation = Some(completed.clone());
                next
            }
            WorkspaceCleanupProgress::RuntimeRemoved => {
                let mut next = state.advance(WorkspaceCleanupProgress::Complete)?;
                next.terminal_workspace_revision =
                    Some(publication::finish(self, &database, &state, workspace, session).await?);
                next
            }
            _ => return Err(CleanupError::Conflict.into()),
        };
        next.seal()?;
        let row = read(&database, WORKSPACE_TOMBSTONES, state.id(), session).await?;
        if records::read_workspace_cleanup_state(&row)? != state {
            return Err(CleanupError::Conflict.into());
        }
        let changed = majority_collection::<Document>(&database, WORKSPACE_TOMBSTONES)
            .update_one(row, doc! {"$set":records::state_document(&next)?})
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(CleanupError::Conflict.into());
        }
        records::append_audit(&database, &next, session).await?;
        permit.require_current()?;
        Ok(next)
    }
}
