//! Ending and removing sessions.
//!
//! Split from the service so that file stays about running sessions and their
//! terminals. The distinction these two methods draw is the point: terminating
//! stops the runtime and leaves the session readable, deleting is the owner
//! saying they are finished with it.

use layer3_adapter::RuntimeHandle;

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::{SessionId, SubjectId};
use crate::log::append;
use crate::service::{store_failed, SessionService};
use crate::session::{RecoveryState, SessionStatus, StopReason};
use crate::store::SessionRecord;

impl SessionService {
    /// Fail unless this caller may end or pause this particular session.
    ///
    /// One place rather than repeated at each call site, so that stopping and
    /// terminating can never drift into holding different opinions about who
    /// may do which. An operator acting on work that is not theirs is logged
    /// at warning: it is legitimate, it is rare, and it is exactly the line in
    /// a log somebody will later want to find.
    fn authorize_control(
        &self,
        caller: &Caller,
        record: &SessionRecord,
        operation: &'static str,
    ) -> Result<(), CoreError> {
        let owner = &record.session.owner;
        caller.require_control_of(owner)?;
        if caller.is_acting_for_another(owner) {
            tracing::warn!(
                session_id = %record.session.id,
                actor = %caller.subject,
                %owner,
                operation,
                "a session is being acted on by somebody other than its owner"
            );
        }
        Ok(())
    }

    /// End a session and tear its runtime down.
    ///
    /// Works whether or not this process is the one that started the session.
    /// A sandbox outlives the process that provisioned it, so after a restart
    /// the durable record is the only route to it — and refusing to terminate
    /// there would leave a runtime nobody can reach still consuming resources.
    ///
    /// An operator may end a session they do not own. That is a real power and
    /// it is checked in two stages: entitlement to end anything at all before
    /// the record is read, and entitlement to end *this* one once its owner is
    /// known.
    pub async fn terminate_session(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<(), CoreError> {
        caller.require_any_control()?;
        self.end_session(caller, id).await.map(|_| ())
    }

    /// End a session as an operator, whoever owns it.
    ///
    /// Separate from [`Self::terminate_session`] only in the gate it opens
    /// with: the everyday grant does not reach this one, so a caller arriving
    /// here is exercising the operator capability whatever the session turns
    /// out to be. That is what makes it worth having its own entry point —
    /// the surface projecting it can be restricted as an operator surface, and
    /// a record of a call to it means what it appears to mean.
    ///
    /// Returns the subject whose work was ended, which the caller needs in
    /// order to write down whose it was.
    pub async fn terminate_any_session(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<SubjectId, CoreError> {
        caller
            .authorization
            .require(Capability::ControlAnySession)?;
        self.end_session(caller, id).await
    }

    async fn end_session(&self, caller: &Caller, id: &SessionId) -> Result<SubjectId, CoreError> {
        let record = self.record(id).await?;
        self.authorize_control(caller, &record, "terminate")?;
        let owner = record.session.owner.clone();
        let adapter = self.adapters.get(&record.session.kernel).cloned();

        // A live entry is the fast path; a record alone is the post-restart
        // path. Both must reach the same end state.
        let live_runtime = {
            let mut entries = self.entries.write().await;
            match entries.get_mut(id) {
                Some(entry) => {
                    entry.session.status = SessionStatus::Terminated;
                    entry.session.stop_reason = Some(StopReason::UserTerminated);
                    entry.session.recovery_state = RecoveryState::Retired;
                    append(
                        entry,
                        EventKind::StatusChanged {
                            status: SessionStatus::Terminated,
                            stop_reason: Some(StopReason::UserTerminated),
                            detail: None,
                        },
                    );
                    entry.input = None;
                    Some(entry.runtime.take())
                }
                None => None,
            }
        };

        let runtime = match live_runtime {
            Some(taken) => {
                self.flush(id).await;
                taken
            }
            None => {
                // No live entry: write the terminal state straight through, and
                // reconstruct the runtime handle from what was persisted.
                self.retire_record(&record).await?;
                record.runtime_id.clone().map(RuntimeHandle)
            }
        };

        if let (Some(adapter), Some(handle)) = (adapter, runtime) {
            if let Err(e) = adapter.terminate(&handle).await {
                // The session is already marked terminated; a runtime that
                // refuses teardown is logged, never silently swallowed.
                tracing::warn!(session_id = %id, error = %e, "runtime teardown failed");
            }
        }
        // The credential goes with the session. Nothing will present it
        // again, and one nobody is watching is the kind worth taking back.
        if let Some(source) = self.credentials.as_ref() {
            source.release(id).await;
        }
        tracing::info!(session_id = %id, "session terminated");
        Ok(owner)
    }

    /// Write a session straight to its terminal state in the store.
    ///
    /// The path for a session with no live entry, where there is no buffered
    /// event list to append to and `flush` has nothing to read.
    async fn retire_record(&self, record: &SessionRecord) -> Result<(), CoreError> {
        let mut retired = record.clone();
        retired.session.status = SessionStatus::Terminated;
        retired.session.stop_reason = Some(StopReason::UserTerminated);
        retired.session.recovery_state = RecoveryState::Retired;
        retired.session.last_seq =
            self.sequences
                .take(&retired.session.id, retired.session.last_seq, 1);
        // The runtime is being torn down, so the record must stop pointing at
        // it or a later sweep would try to reclaim a sandbox that is gone.
        retired.runtime_id = None;
        self.store.upsert(&retired).await.map_err(store_failed)?;
        self.store
            .append_events(&[Event {
                session_id: retired.session.id.clone(),
                seq: retired.session.last_seq,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Terminated,
                    stop_reason: Some(StopReason::UserTerminated),
                    detail: None,
                },
            }])
            .await
            .map_err(store_failed)?;
        Ok(())
    }

    /// Stop a session's runtime, keeping the session.
    ///
    /// The reversible half of ending something. The sandbox goes, so nothing
    /// is being paid for and nothing is running, while the session stays
    /// listed and can be picked up again — which is the difference between
    /// putting work down and throwing it away.
    pub async fn stop_session(&self, caller: &Caller, id: &SessionId) -> Result<(), CoreError> {
        caller.require_any_control()?;
        self.rest_session(caller, id).await.map(|_| ())
    }

    /// Stop a session's runtime as an operator, whoever owns it.
    ///
    /// The reversible control, on the same terms as
    /// [`Self::terminate_any_session`]: the everyday grant does not open it,
    /// and it reports whose work was stopped.
    pub async fn stop_any_session(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<SubjectId, CoreError> {
        caller
            .authorization
            .require(Capability::ControlAnySession)?;
        self.rest_session(caller, id).await
    }

    async fn rest_session(&self, caller: &Caller, id: &SessionId) -> Result<SubjectId, CoreError> {
        let record = self.record(id).await?;
        self.authorize_control(caller, &record, "stop")?;
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        let owner = record.session.owner.clone();

        let adapter = self.adapters.get(&record.session.kernel).cloned();
        let runtime = {
            let mut entries = self.entries.write().await;
            match entries.get_mut(id) {
                Some(entry) => {
                    entry.session.status = SessionStatus::Idle;
                    entry.session.stop_reason = Some(StopReason::UserTerminated);
                    entry.input = None;
                    let taken = entry.runtime.take();
                    append(
                        entry,
                        EventKind::StatusChanged {
                            status: SessionStatus::Idle,
                            stop_reason: Some(StopReason::UserTerminated),
                            detail: None,
                        },
                    );
                    Some(taken)
                }
                None => None,
            }
        };

        let handle = match runtime {
            Some(taken) => {
                // Captured before the entry goes: this is the last chance to
                // store what the session showed, and a stopped session with no
                // transcript is one whose history was thrown away by stopping
                // it — which is exactly what stopping is meant not to do.
                self.capture_transcript(id).await;
                self.flush(id).await;
                // The attachment goes with the runtime: a live entry pointing
                // at a torn-down sandbox would accept input that reaches
                // nothing.
                self.entries.write().await.remove(id);
                taken
            }
            None => {
                self.rest_record(&record).await?;
                record.runtime_id.clone().map(RuntimeHandle)
            }
        };

        if let (Some(adapter), Some(handle)) = (adapter, handle) {
            if let Err(e) = adapter.terminate(&handle).await {
                tracing::warn!(session_id = %id, error = %e, "runtime teardown failed");
            }
        }
        tracing::info!(session_id = %id, "session stopped");
        Ok(owner)
    }

    /// Write a session to its stopped state in the store.
    async fn rest_record(&self, record: &SessionRecord) -> Result<(), CoreError> {
        let mut resting = record.clone();
        resting.session.status = SessionStatus::Idle;
        resting.session.stop_reason = Some(StopReason::UserTerminated);
        resting.session.last_seq =
            self.sequences
                .take(&resting.session.id, resting.session.last_seq, 1);
        // Nothing is running for it any more, so nothing should claim to be.
        resting.runtime_id = None;
        self.store.upsert(&resting).await.map_err(store_failed)?;
        self.store
            .append_events(&[Event {
                session_id: resting.session.id.clone(),
                seq: resting.session.last_seq,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Idle,
                    stop_reason: Some(StopReason::UserTerminated),
                    detail: None,
                },
            }])
            .await
            .map_err(store_failed)?;
        Ok(())
    }

    /// Remove a session permanently.
    ///
    /// Distinct from ending one. Terminating stops the runtime and leaves the
    /// session listed, because a finished conversation is still worth reading;
    /// deleting is the owner saying they are done with it. A session still
    /// running is terminated first, so deleting never strands a sandbox.
    ///
    /// Returns the context reference, if the session had one, so the caller can
    /// see what was removed alongside the record.
    ///
    /// Unlike stopping and terminating, this stays with the owner. An operator
    /// needs to be able to stop work that is costing money or misbehaving, and
    /// stopping is reversible; destroying somebody else's history is not, and
    /// nothing about running the service requires it.
    pub async fn delete_session(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<Option<String>, CoreError> {
        caller
            .authorization
            .require(Capability::TerminateOwnSession)?;
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if !record.session.status.is_terminal() {
            self.terminate_session(caller, id).await?;
        }
        self.entries.write().await.remove(id);
        self.store.delete(id).await.map_err(store_failed)?;
        // The events go with the session, so the position kept for it is a
        // number held for something nothing can read. Only here: a session
        // that merely stopped keeps its position, because it can be revived
        // and its log continues rather than starting again.
        self.sequences.forget(id);

        // The transcript goes with the session. Leaving it behind would keep a
        // record of work its owner asked to be removed, and orphan a blob
        // nothing references.
        if let (Some(store), Some(reference)) = (&self.context, record.context_ref.as_deref()) {
            if let Err(e) = store.delete(reference).await {
                tracing::error!(session_id = %id, error = %e, "cannot remove stored context");
            }
        }
        tracing::info!(session_id = %id, "session deleted");
        Ok(record.context_ref)
    }
}
