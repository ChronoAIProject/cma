//! Ending and removing sessions.
//!
//! Split from the service so that file stays about running sessions and their
//! terminals. The distinction these two methods draw is the point: terminating
//! stops the runtime and leaves the session readable, deleting is the owner
//! saying they are finished with it.

use std::sync::Arc;

use layer3_adapter::RuntimeHandle;

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::{SessionId, SubjectId};
use crate::log::{append_with, drop_undelivered, ignore_rejected};
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
        self: &Arc<Self>,
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
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<SubjectId, CoreError> {
        caller
            .authorization
            .require(Capability::ControlAnySession)?;
        self.end_session(caller, id).await
    }

    async fn end_session(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<SubjectId, CoreError> {
        let lifecycle_gate = self.lifecycle_gates.gate(id);
        let lifecycle = lifecycle_gate.lock_owned().await;
        let record = self.record(id).await?;
        self.authorize_control(caller, &record, "terminate")?;
        let owner = record.session.owner.clone();
        if record.session.status.is_terminal() {
            return Ok(owner);
        }
        let adapter = self.adapters.get(&record.session.kernel).cloned();

        // A live entry is the fast path; a record alone is the post-restart
        // path. Both must reach the same end state. The live proposal changes
        // no process state until its exact record/event unit is durable.
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(id) {
            let service = Arc::clone(self);
            let session_id = id.clone();
            let live_owner = owner.clone();
            let live_adapter = adapter.clone();
            let fallback_runtime = entry.runtime.clone();
            let entry_screen = Arc::clone(&entry.screen);
            let rejected_service = Arc::clone(self);
            let rejected_id = id.clone();
            let rejected_screen = Arc::clone(&entry_screen);
            let commit = append_with(
                entry,
                EventKind::StatusChanged {
                    status: SessionStatus::Terminated,
                    stop_reason: Some(StopReason::UserTerminated),
                    detail: None,
                    capability: None,
                },
                |intended| {
                    intended.session.status = SessionStatus::Terminated;
                    intended.session.stop_reason = Some(StopReason::UserTerminated);
                    intended.session.recovery_state = RecoveryState::Retired;
                    intended.runtime_id = None;
                },
                move |committed| async move {
                    let current_runtime = {
                        let mut entries = service.entries.write().await;
                        entries
                            .get_mut(&session_id)
                            .filter(|entry| Arc::ptr_eq(&entry.screen, &entry_screen))
                            .map(|entry| {
                                entry.session.status = committed.session.status;
                                entry.session.stop_reason = committed.session.stop_reason;
                                entry.session.recovery_state = committed.session.recovery_state;
                                entry.session.last_seq = committed.session.last_seq;
                                let event_log = Arc::clone(&entry.event_log);
                                event_log.synchronize_tail(&mut entry.session.last_seq);
                                entry.input = None;
                                entry.conversation = None;
                                entry.runtime.take()
                            })
                    };
                    let installed = current_runtime.is_some();
                    let mut runtimes: Vec<RuntimeHandle> = fallback_runtime.into_iter().collect();
                    if let Some(Some(current)) = current_runtime {
                        if !runtimes.iter().any(|runtime| runtime.0 == current.0) {
                            runtimes.push(current);
                        }
                    }
                    if let Some(adapter) = live_adapter {
                        for handle in runtimes {
                            if let Err(error) = adapter.terminate(&handle).await {
                                tracing::warn!(session_id = %session_id, error = %error, "runtime teardown failed");
                            }
                        }
                    }
                    if let Some(source) = service.credentials.as_ref() {
                        source.release(&session_id).await;
                    }
                    tracing::info!(session_id = %session_id, "session terminated");
                    (
                        installed
                            .then_some(live_owner)
                            .ok_or(CoreError::SessionNotFound),
                        lifecycle,
                    )
                },
                move |_error| async move {
                    rejected_service
                        .synchronize_live_tail_for(&rejected_id, &rejected_screen)
                        .await;
                },
                drop_undelivered,
            );
            drop(entries);
            return commit.committed().await.map_err(store_failed)?;
        }
        drop(entries);

        // No live entry: retain the persisted runtime handle separately from
        // the proposal that clears it, then tear it down on eventual success.
        let mut retired = record.clone();
        retired.session.status = SessionStatus::Terminated;
        retired.session.stop_reason = Some(StopReason::UserTerminated);
        retired.session.recovery_state = RecoveryState::Retired;
        // The runtime is being torn down, so the record must stop pointing at
        // it or a later sweep would try to reclaim a sandbox that is gone.
        retired.runtime_id = None;
        let runtime = record.runtime_id.clone().map(RuntimeHandle);
        let service = Arc::clone(self);
        let session_id = id.clone();
        self.persist_durably_with(
            retired,
            vec![Event {
                session_id: session_id.clone(),
                seq: 0,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Terminated,
                    stop_reason: Some(StopReason::UserTerminated),
                    detail: None,
                    capability: None,
                },
                appended_at: String::new(),
            }],
            move |_committed| async move {
                if let (Some(adapter), Some(handle)) = (adapter, runtime) {
                    if let Err(error) = adapter.terminate(&handle).await {
                        tracing::warn!(session_id = %session_id, error = %error, "runtime teardown failed");
                    }
                }
                if let Some(source) = service.credentials.as_ref() {
                    source.release(&session_id).await;
                }
                tracing::info!(session_id = %session_id, "session terminated");
                (owner, lifecycle)
            },
            ignore_rejected,
            drop_undelivered,
        )
        .committed()
        .await
        .map_err(store_failed)
    }

    /// Stop a session's runtime, keeping the session.
    ///
    /// The reversible half of ending something. The sandbox goes, so nothing
    /// is being paid for and nothing is running, while the session stays
    /// listed and can be picked up again — which is the difference between
    /// putting work down and throwing it away.
    pub async fn stop_session(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<(), CoreError> {
        caller.require_any_control()?;
        self.rest_session(caller, id).await.map(|_| ())
    }

    /// Stop a session's runtime as an operator, whoever owns it.
    ///
    /// The reversible control, on the same terms as
    /// [`Self::terminate_any_session`]: the everyday grant does not open it,
    /// and it reports whose work was stopped.
    pub async fn stop_any_session(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<SubjectId, CoreError> {
        caller
            .authorization
            .require(Capability::ControlAnySession)?;
        self.rest_session(caller, id).await
    }

    async fn rest_session(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<SubjectId, CoreError> {
        let lifecycle_gate = self.lifecycle_gates.gate(id);
        let lifecycle = lifecycle_gate.lock_owned().await;
        let record = self.record(id).await?;
        self.authorize_control(caller, &record, "stop")?;
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        let owner = record.session.owner.clone();

        let adapter = self.adapters.get(&record.session.kernel).cloned();
        // Captured before constructing the semantic write: its context
        // reference then travels in the same awaited record as the stopped
        // state instead of being left to a later best-effort flush.
        if self.entries.read().await.contains_key(id) {
            self.capture_transcript(id).await;
        }
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(id) {
            let service = Arc::clone(self);
            let session_id = id.clone();
            let live_owner = owner.clone();
            let live_adapter = adapter.clone();
            let fallback_runtime = entry.runtime.clone();
            let entry_screen = Arc::clone(&entry.screen);
            let rejected_service = Arc::clone(self);
            let rejected_id = id.clone();
            let rejected_screen = Arc::clone(&entry_screen);
            let commit = append_with(
                entry,
                EventKind::StatusChanged {
                    status: SessionStatus::Idle,
                    stop_reason: Some(StopReason::UserTerminated),
                    detail: None,
                    capability: None,
                },
                |intended| {
                    intended.session.status = SessionStatus::Idle;
                    intended.session.stop_reason = Some(StopReason::UserTerminated);
                    intended.session.recovery_state = RecoveryState::Idle;
                    intended.runtime_id = None;
                },
                move |committed| async move {
                    // The attachment goes with the runtime. Removal follows
                    // durability, while the retained task owns teardown even
                    // if the original request is gone.
                    let removed = {
                        let mut entries = service.entries.write().await;
                        let is_original = entries
                            .get(&session_id)
                            .is_some_and(|entry| Arc::ptr_eq(&entry.screen, &entry_screen));
                        is_original.then(|| entries.remove(&session_id)).flatten()
                    };
                    let installed = removed.is_some();
                    let current_runtime = removed.and_then(|mut entry| {
                        entry.session.status = committed.session.status;
                        entry.session.stop_reason = committed.session.stop_reason;
                        entry.session.recovery_state = committed.session.recovery_state;
                        entry.session.last_seq = committed.session.last_seq;
                        entry.input = None;
                        entry.conversation = None;
                        entry.runtime.take()
                    });
                    let mut runtimes: Vec<RuntimeHandle> = fallback_runtime.into_iter().collect();
                    if let Some(current) = current_runtime {
                        if !runtimes.iter().any(|runtime| runtime.0 == current.0) {
                            runtimes.push(current);
                        }
                    }
                    if let Some(adapter) = live_adapter {
                        for handle in runtimes {
                            if let Err(error) = adapter.terminate(&handle).await {
                                tracing::warn!(session_id = %session_id, error = %error, "runtime teardown failed");
                            }
                        }
                    }
                    tracing::info!(session_id = %session_id, "session stopped");
                    (
                        installed
                            .then_some(live_owner)
                            .ok_or(CoreError::SessionNotFound),
                        lifecycle,
                    )
                },
                move |_error| async move {
                    rejected_service
                        .synchronize_live_tail_for(&rejected_id, &rejected_screen)
                        .await;
                },
                drop_undelivered,
            );
            drop(entries);
            return commit.committed().await.map_err(store_failed)?;
        }
        drop(entries);

        // Match termination's installation boundary for a post-restart
        // session and retain its handle before the record clears it.
        let mut resting = record.clone();
        resting.session.status = SessionStatus::Idle;
        resting.session.stop_reason = Some(StopReason::UserTerminated);
        resting.session.recovery_state = RecoveryState::Idle;
        // Nothing is running for it any more, so nothing should claim to be.
        resting.runtime_id = None;
        let runtime = record.runtime_id.clone().map(RuntimeHandle);
        let session_id = id.clone();
        self.persist_durably_with(
            resting,
            vec![Event {
                session_id: session_id.clone(),
                seq: 0,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Idle,
                    stop_reason: Some(StopReason::UserTerminated),
                    detail: None,
                    capability: None,
                },
                appended_at: String::new(),
            }],
            move |_committed| async move {
                if let (Some(adapter), Some(handle)) = (adapter, runtime) {
                    if let Err(error) = adapter.terminate(&handle).await {
                        tracing::warn!(session_id = %session_id, error = %error, "runtime teardown failed");
                    }
                }
                tracing::info!(session_id = %session_id, "session stopped");
                (owner, lifecycle)
            },
            ignore_rejected,
            drop_undelivered,
        )
        .committed()
        .await
        .map_err(store_failed)
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
        self: &Arc<Self>,
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
        self.delete_durably(id).await.map_err(store_failed)?;
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
