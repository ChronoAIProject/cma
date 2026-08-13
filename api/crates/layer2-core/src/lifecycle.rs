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
use crate::ids::SessionId;
use crate::service::{append, store_failed, SessionService};
use crate::session::{RecoveryState, SessionStatus, StopReason};
use crate::store::SessionRecord;

impl SessionService {
    /// End a session and tear its runtime down.
    ///
    /// Works whether or not this process is the one that started the session.
    /// A sandbox outlives the process that provisioned it, so after a restart
    /// the durable record is the only route to it — and refusing to terminate
    /// there would leave a runtime nobody can reach still consuming resources.
    pub async fn terminate_session(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<(), CoreError> {
        caller
            .authorization
            .require(Capability::TerminateOwnSession)?;
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
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
        tracing::info!(session_id = %id, "session terminated");
        Ok(())
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
        retired.session.last_seq += 1;
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
