//! Ending and removing sessions.
//!
//! Split from the service so that file stays about running sessions and their
//! terminals. The distinction these two methods draw is the point: terminating
//! stops the runtime and leaves the session readable, deleting is the owner
//! saying they are finished with it.

use std::sync::Arc;

use layer3_adapter::{KernelAdapter, RuntimeHandle};

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::{SessionId, SubjectId};
use crate::log::{
    append_terminal_lifecycle_repair, append_with_rotation_claim, drop_undelivered, ignore_rejected,
};
use crate::service::{store_failed, SessionService};
use crate::session::{RecoveryState, SessionStatus, StopReason};
use crate::store::{SessionRecord, StoreError};

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

    /// Tear down every compatibility handle while the durable one remains retryable.
    ///
    /// A live entry can lag a published rotation and still name H1 while the
    /// durable row names H2. H1 has no other durable home, so delete it first but
    /// leave it installed locally until the whole operation succeeds. H2 goes
    /// through the exact durable clear helper so a provider or store failure
    /// leaves enough authority for a later lifecycle retry.
    async fn teardown_compatibility_lifecycle_runtimes(
        &self,
        adapter: Option<&Arc<dyn KernelAdapter>>,
        claimed: &SessionRecord,
        local_runtimes: &[RuntimeHandle],
        operation: &'static str,
    ) -> Result<SessionRecord, CoreError> {
        if claimed.runtime_id.is_none() && local_runtimes.is_empty() {
            return Ok(claimed.clone());
        }
        let adapter = adapter.ok_or_else(|| {
            CoreError::RuntimeUnavailable(format!(
                "{operation} cannot tear down its compatibility runtime because the kernel adapter is unavailable"
            ))
        })?;
        self.require_compatibility_provider_mutation(&claimed.session.id, operation)
            .await?;

        for (index, handle) in local_runtimes.iter().enumerate() {
            if claimed.runtime_id.as_deref() == Some(handle.0.as_str())
                || local_runtimes[..index]
                    .iter()
                    .any(|prior| prior.0 == handle.0)
            {
                continue;
            }
            adapter.terminate(handle).await.map_err(|error| {
                CoreError::RuntimeUnavailable(format!(
                    "{operation} could not tear down runtime {}: {error}",
                    handle.0
                ))
            })?;
        }

        if claimed.runtime_id.is_some() {
            self.teardown_compatibility_runtime(adapter, claimed, operation)
                .await
        } else {
            Ok(claimed.clone())
        }
    }

    async fn await_compatibility_lifecycle_task(
        task: tokio::task::JoinHandle<Result<SubjectId, CoreError>>,
        operation: &'static str,
    ) -> Result<SubjectId, CoreError> {
        task.await.map_err(|error| {
            CoreError::RuntimeUnavailable(format!(
                "{operation} compatibility cleanup task stopped: {error}"
            ))
        })?
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
        self.end_session(caller, id, false).await.map(|_| ())
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
        self.end_session(caller, id, true).await
    }

    async fn end_session(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        operator: bool,
    ) -> Result<SubjectId, CoreError> {
        let lifecycle = self.lock_lifecycle(id).await;
        let record = self.record(id).await?;
        self.authorize_control(caller, &record, "terminate")?;
        if record.chat_origin.is_some() {
            if !operator {
                return Err(CoreError::chat_lifecycle_managed("termination"));
            }
            return self
                .set_chat_thread_lifecycle(
                    record,
                    SessionStatus::Terminated,
                    RecoveryState::Retired,
                    "operator terminated this platform-managed chat thread",
                )
                .await;
        }
        let owner = record.session.owner.clone();
        let lease = self
            .acquire_runtime_lease_at(&record, crate::coordination::SessionOperation::Lifecycle)
            .await
            .map_err(store_failed)?;
        if let Some(lease) = lease {
            return self.end_session_coordinated(record, lease, lifecycle).await;
        }
        let has_local_entry = self.entries.read().await.contains_key(id);
        if record.session.status.is_terminal()
            && record.rotation_claim.is_none()
            && record.pending_handover.is_none()
            && record.runtime_id.is_none()
            && !has_local_entry
        {
            let credentials = self.credentials.clone();
            let service = Arc::clone(self);
            let session_id = id.clone();
            let task = tokio::spawn(async move {
                let _lifecycle = lifecycle;
                if let Some(source) = credentials {
                    if let Err(error) = source.release(&session_id).await {
                        tracing::error!(
                            session_id = %session_id,
                            error = %error,
                            "could not repair terminal session credential custody"
                        );
                    }
                }
                service.finish_skill_pack_cleanup_after_commit(session_id);
                Ok(owner)
            });
            return Self::await_compatibility_lifecycle_task(task, "session termination").await;
        }
        self.require_compatibility_provider_mutation(id, "termination")
            .await?;
        let adapter = self.adapters.get(&record.session.kernel).cloned();
        let phase_one_complete = record.session.status == SessionStatus::Terminated
            && record.session.stop_reason == Some(StopReason::UserTerminated)
            && record.session.recovery_state == RecoveryState::Retired
            && record.rotation_claim.is_none()
            && record.pending_handover.is_none();

        // A live entry is the fast path; a record alone is the post-restart
        // path. Both must reach the same end state. The live proposal changes
        // no process state until its exact record/event unit is durable.
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(id) {
            let service = Arc::clone(self);
            let session_id = id.clone();
            let live_owner = owner.clone();
            let fallback_runtime = entry.runtime.clone();
            let live_adapter = adapter.clone();
            let entry_screen = Arc::clone(&entry.screen);
            let rejected_service = Arc::clone(self);
            let rejected_id = id.clone();
            let rejected_screen = Arc::clone(&entry_screen);
            let continuation = move |committed: SessionRecord| async move {
                let (installed, local_runtimes) = {
                    let mut entries = service.entries.write().await;
                    let mut runtimes: Vec<RuntimeHandle> = fallback_runtime.into_iter().collect();
                    let installed = entries
                        .get_mut(&session_id)
                        .filter(|entry| Arc::ptr_eq(&entry.screen, &entry_screen))
                        .is_some_and(|entry| {
                            entry.session.status = committed.session.status;
                            entry.session.stop_reason = committed.session.stop_reason;
                            entry.session.recovery_state = committed.session.recovery_state;
                            entry.session.last_seq = committed.session.last_seq;
                            let event_log = Arc::clone(&entry.event_log);
                            event_log.synchronize_tail(&mut entry.session.last_seq);
                            entry.input = None;
                            entry.conversation = None;
                            if let Some(runtime) = entry.runtime.clone() {
                                if !runtimes.iter().any(|known| known.0 == runtime.0) {
                                    runtimes.push(runtime);
                                }
                            }
                            true
                        });
                    (installed, runtimes)
                };
                let task = tokio::spawn(async move {
                    let _lifecycle = lifecycle;
                    let cleanup = service
                        .teardown_compatibility_lifecycle_runtimes(
                            live_adapter.as_ref(),
                            &committed,
                            &local_runtimes,
                            "session termination",
                        )
                        .await;
                    if cleanup.is_ok() {
                        if let Some(entry) = service
                            .entries
                            .write()
                            .await
                            .get_mut(&session_id)
                            .filter(|entry| Arc::ptr_eq(&entry.screen, &entry_screen))
                        {
                            entry.runtime = None;
                        }
                        if let Some(source) = service.credentials.as_ref() {
                            if let Err(error) = source.release(&session_id).await {
                                tracing::error!(
                                    session_id = %session_id,
                                    error = %error,
                                    "could not retire this session's credential custody"
                                );
                            }
                        }
                        service.finish_skill_pack_cleanup_after_commit(session_id.clone());
                        tracing::info!(session_id = %session_id, "session terminated");
                    }
                    cleanup.and_then(|_| {
                        installed
                            .then_some(live_owner)
                            .ok_or(CoreError::SessionNotFound)
                    })
                });
                (task, ())
            };
            let on_rejected = move |_error| async move {
                rejected_service
                    .synchronize_ordered_live_tail_for(&rejected_id, &rejected_screen)
                    .await;
            };
            let commit = if phase_one_complete {
                self.persist_terminal_lifecycle_repair_with(
                    &record,
                    record.clone(),
                    Vec::new(),
                    continuation,
                    on_rejected,
                    drop_undelivered,
                )
            } else {
                append_terminal_lifecycle_repair(
                    entry,
                    &record,
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
                        intended.rotation_claim = None;
                        intended.pending_handover = None;
                    },
                    continuation,
                    on_rejected,
                    drop_undelivered,
                )
            };
            drop(entries);
            let task = commit.committed().await.map_err(store_failed)?;
            return Self::await_compatibility_lifecycle_task(task, "session termination").await;
        }
        drop(entries);

        // No live entry: phase one retains the exact runtime until provider
        // teardown and the conditional durable clear both succeed.
        let mut retired = record.clone();
        retired.session.status = SessionStatus::Terminated;
        retired.session.stop_reason = Some(StopReason::UserTerminated);
        retired.session.recovery_state = RecoveryState::Retired;
        retired.rotation_claim = None;
        retired.pending_handover = None;
        let service = Arc::clone(self);
        let session_id = id.clone();
        let events = if phase_one_complete {
            Vec::new()
        } else {
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
            }]
        };
        let task = self
            .persist_terminal_lifecycle_repair_with(
                &record,
                retired,
                events,
                move |committed| async move {
                    let task = tokio::spawn(async move {
                        let _lifecycle = lifecycle;
                        let result = service
                            .teardown_compatibility_lifecycle_runtimes(
                                adapter.as_ref(),
                                &committed,
                                &[],
                                "session termination",
                            )
                            .await
                            .map(|_| owner);
                        if result.is_ok() {
                            if let Some(source) = service.credentials.as_ref() {
                                if let Err(error) = source.release(&session_id).await {
                                    tracing::error!(
                                        session_id = %session_id,
                                        error = %error,
                                        "could not retire this session's credential custody"
                                    );
                                }
                            }
                            service.finish_skill_pack_cleanup_after_commit(session_id.clone());
                            tracing::info!(session_id = %session_id, "session terminated");
                        }
                        result
                    });
                    (task, ())
                },
                ignore_rejected,
                drop_undelivered,
            )
            .committed()
            .await
            .map_err(store_failed)?;
        Self::await_compatibility_lifecycle_task(task, "session termination").await
    }

    /// End a coordinated session without ever deleting by provider handle alone.
    ///
    /// Current authority is cleared atomically into exact cleanup debt before
    /// the terminal status is committed. Already-terminal rows take this path
    /// too, which lets an idempotent retry finish authority or cleanup work left
    /// by a process that died between those commits.
    async fn end_session_coordinated(
        self: &Arc<Self>,
        record: SessionRecord,
        mut lease: crate::coordination::SessionLease,
        lifecycle: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<SubjectId, CoreError> {
        let id = record.session.id.clone();
        let owner = record.session.owner.clone();
        let adapter = self.adapters.get(&record.session.kernel).cloned();
        if self.entries.read().await.contains_key(&id) {
            self.capture_transcript(&id).await;
        }
        let input_fence = self.input_gates.gate(&id).write_owned().await;

        if record.runtime_id.is_some() {
            if let Err(error) = self.retire_current_runtime(&mut lease).await {
                if !self.confirm_retired_lifecycle_authority(&mut lease).await {
                    // The store result is unknown. Keeping a live entry would
                    // let input reach a runtime whose authority may already be
                    // retired, so fail closed until reconciliation can read it.
                    self.entries.write().await.remove(&id);
                    drop(input_fence);
                    let _ = self.store.release_session_lease(&lease).await;
                    drop(lifecycle);
                    return Err(store_failed(error));
                }
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "runtime retirement was confirmed after an unknown store result"
                );
            }
        }

        let mut terminal = match self.store.get(&id).await {
            Ok(Some(terminal)) if terminal.runtime_id.is_none() => terminal,
            Ok(Some(_)) => {
                self.entries.write().await.remove(&id);
                drop(input_fence);
                let _ = self.store.release_session_lease(&lease).await;
                drop(lifecycle);
                return Err(CoreError::RuntimeUnavailable(
                    "runtime authority changed while the session was ending".into(),
                ));
            }
            Ok(None) => {
                self.entries.write().await.remove(&id);
                drop(input_fence);
                let _ = self.store.release_session_lease(&lease).await;
                drop(lifecycle);
                return Err(CoreError::SessionNotFound);
            }
            Err(error) => {
                self.entries.write().await.remove(&id);
                drop(input_fence);
                let _ = self.store.release_session_lease(&lease).await;
                drop(lifecycle);
                return Err(store_failed(error));
            }
        };

        let lifecycle_changed = !terminal.session.status.is_terminal()
            || terminal.session.stop_reason != Some(StopReason::UserTerminated)
            || terminal.session.recovery_state != RecoveryState::Retired
            || terminal.rotation_claim.is_some()
            || terminal.pending_handover.is_some();
        let terminal_custody = match (&terminal.skill_packs, self.skill_packs.as_ref()) {
            (crate::store::SkillPackState::Cleaned, _) => crate::store::SkillPackState::Cleaned,
            (crate::store::SkillPackState::Staged(packs), None) if packs.is_empty() => {
                crate::store::SkillPackState::Cleaned
            }
            (
                crate::store::SkillPackState::LegacyHandover {
                    stored_digest: None,
                    ..
                },
                None,
            ) => crate::store::SkillPackState::Cleaned,
            (_, _) => crate::store::SkillPackState::CleanupPending,
        };
        if lifecycle_changed {
            let claimed_terminal = terminal.clone();
            terminal.session.status = SessionStatus::Terminated;
            terminal.session.stop_reason = Some(StopReason::UserTerminated);
            terminal.session.recovery_state = RecoveryState::Retired;
            terminal.rotation_claim = None;
            terminal.pending_handover = None;
            terminal.skill_packs = terminal_custody;
            if let Err(error) = self
                .terminate_with_skill_cleanup_durably(
                    &mut terminal,
                    &claimed_terminal,
                    vec![Event {
                        session_id: id.clone(),
                        seq: 0,
                        kind: EventKind::StatusChanged {
                            status: SessionStatus::Terminated,
                            stop_reason: Some(StopReason::UserTerminated),
                            detail: None,
                            capability: None,
                        },
                        appended_at: String::new(),
                    }],
                )
                .await
            {
                self.entries.write().await.remove(&id);
                drop(input_fence);
                let _ = self.store.release_session_lease(&lease).await;
                drop(lifecycle);
                return Err(store_failed(error));
            }
        } else if terminal.skill_packs != terminal_custody {
            if let Err(error) = self
                .persist_skill_pack_state_durably(&mut terminal, terminal_custody)
                .await
            {
                self.entries.write().await.remove(&id);
                drop(input_fence);
                let _ = self.store.release_session_lease(&lease).await;
                drop(lifecycle);
                return Err(store_failed(error));
            }
        }

        let boundary = crate::coordination::SessionBoundary::for_record(
            &terminal,
            lease.boundary.authority.generation,
        );
        if boundary != lease.boundary {
            if let Err(error) = self.advance_runtime_lease(&mut lease, &boundary).await {
                self.entries.write().await.remove(&id);
                drop(input_fence);
                let _ = self.store.release_session_lease(&lease).await;
                drop(lifecycle);
                return Err(store_failed(error));
            }
        }

        if let Some(entry) = self.entries.write().await.get_mut(&id) {
            entry.session.status = terminal.session.status;
            entry.session.stop_reason = terminal.session.stop_reason;
            entry.session.recovery_state = terminal.session.recovery_state;
            entry.session.last_seq = terminal.session.last_seq;
            entry.runtime = None;
            entry.input = None;
            entry.conversation = None;
        }
        drop(input_fence);

        if let Some(adapter) = adapter {
            match self.store.runtime_cleanup_debts(&id).await {
                Ok(cleanups) => {
                    for cleanup in cleanups {
                        if let Err(error) = self
                            .reclaim_runtime_with_lease(&adapter, &lease, &cleanup)
                            .await
                        {
                            tracing::warn!(
                                session_id = %id,
                                runtime = %cleanup.runtime_id,
                                generation = cleanup.generation,
                                %error,
                                "terminated runtime cleanup remains durable for retry"
                            );
                        }
                    }
                }
                Err(error) => tracing::warn!(
                    session_id = %id,
                    %error,
                    "cannot enumerate terminated runtime cleanup debt"
                ),
            }
        }
        if let Some(source) = self.credentials.as_ref() {
            if let Err(error) = source.release(&id).await {
                tracing::error!(
                    session_id = %id,
                    %error,
                    "could not retire this session's credential custody"
                );
            }
        }
        if let Err(error) = self
            .finish_skill_pack_cleanup(&mut terminal, Some(&lease))
            .await
        {
            tracing::error!(
                session_id = %id,
                %error,
                "session terminated with skill cleanup still pending"
            );
        }
        if let Err(error) = self.store.release_session_lease(&lease).await {
            tracing::debug!(
                session_id = %id,
                %error,
                "termination lease ended before explicit release"
            );
        }
        drop(lifecycle);
        tracing::info!(session_id = %id, "session terminated");
        Ok(owner)
    }

    /// Resolve an unknown retirement result without inventing a new operation.
    ///
    /// The same lease token can adopt the store's resulting no-runtime boundary
    /// only when that exact token still owns it. A successor lease, changed
    /// status, or surviving runtime all make renewal fail or refuse confirmation.
    pub(crate) async fn confirm_retired_lifecycle_authority(
        &self,
        lease: &mut crate::coordination::SessionLease,
    ) -> bool {
        let record = match self.store.get(&lease.session_id).await {
            Ok(Some(record)) => record,
            _ => return false,
        };
        let authority = match self.store.runtime_authority(&lease.session_id).await {
            Ok(Some(authority)) => authority,
            _ => return false,
        };
        if record.runtime_id.is_some()
            || authority.runtime_id.is_some()
            || authority.generation != lease.boundary.authority.generation
            || record.session.status != lease.boundary.status
        {
            return false;
        }
        let boundary = crate::coordination::SessionBoundary {
            status: record.session.status,
            authority,
        };
        let mut confirmed = lease.clone();
        confirmed.boundary = boundary;
        if self.renew_runtime_lease(Some(&confirmed)).await.is_err() {
            return false;
        }
        *lease = confirmed;
        true
    }

    /// Advance a provider-operation lease with an idempotent retry and boundary receipt.
    pub(crate) async fn advance_runtime_lease(
        &self,
        lease: &mut crate::coordination::SessionLease,
        boundary: &crate::coordination::SessionBoundary,
    ) -> Result<(), StoreError> {
        let error = match self.store.advance_session_lease(lease, boundary).await {
            Ok(()) => return Ok(()),
            Err(StoreError::Unavailable(_)) => {
                match self.store.advance_session_lease(lease, boundary).await {
                    Ok(()) => return Ok(()),
                    Err(error) => error,
                }
            }
            Err(error) => error,
        };

        // A transport error may have hidden a committed advance. Confirm it by
        // renewing this token at the target boundary; an uncommitted advance
        // still has the old held boundary and cannot pass this comparison.
        let mut confirmed = lease.clone();
        confirmed.boundary = boundary.clone();
        if self.renew_runtime_lease(Some(&confirmed)).await.is_ok() {
            *lease = confirmed;
            Ok(())
        } else {
            Err(error)
        }
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
        self.rest_session(caller, id, false).await.map(|_| ())
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
        self.rest_session(caller, id, true).await
    }

    async fn rest_session(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        operator: bool,
    ) -> Result<SubjectId, CoreError> {
        let lifecycle = self.lock_lifecycle(id).await;
        let record = self.record(id).await?;
        self.authorize_control(caller, &record, "stop")?;
        if record.chat_origin.is_some() {
            if !operator {
                return Err(CoreError::chat_lifecycle_managed("stop"));
            }
            return self
                .set_chat_thread_lifecycle(
                    record,
                    SessionStatus::Idle,
                    RecoveryState::Idle,
                    "operator stopped this platform-managed chat thread",
                )
                .await;
        }
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        let lease = self
            .acquire_runtime_lease_at(&record, crate::coordination::SessionOperation::Lifecycle)
            .await
            .map_err(store_failed)?;
        if let Some(lease) = lease {
            return self
                .rest_session_coordinated(record, lease, lifecycle)
                .await;
        }
        let owner = record.session.owner.clone();

        self.require_compatibility_provider_mutation(id, "stop")
            .await?;

        let adapter = self.adapters.get(&record.session.kernel).cloned();
        let phase_one_complete = record.session.status == SessionStatus::Idle
            && record.session.stop_reason == Some(StopReason::UserTerminated)
            && record.session.recovery_state == RecoveryState::Idle
            && record.rotation_claim.is_none();
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
            let fallback_runtime = entry.runtime.clone();
            let live_adapter = adapter.clone();
            let entry_screen = Arc::clone(&entry.screen);
            let rejected_service = Arc::clone(self);
            let rejected_id = id.clone();
            let rejected_screen = Arc::clone(&entry_screen);
            let continuation = move |committed: SessionRecord| async move {
                // Disable both input planes after phase one is durable, but
                // keep the local handle installed until every teardown and
                // the exact durable clear have succeeded.
                let (installed, local_runtimes) = {
                    let mut entries = service.entries.write().await;
                    let mut runtimes: Vec<RuntimeHandle> = fallback_runtime.into_iter().collect();
                    let installed = entries
                        .get_mut(&session_id)
                        .filter(|entry| Arc::ptr_eq(&entry.screen, &entry_screen))
                        .is_some_and(|entry| {
                            entry.session.status = committed.session.status;
                            entry.session.stop_reason = committed.session.stop_reason;
                            entry.session.recovery_state = committed.session.recovery_state;
                            entry.session.last_seq = committed.session.last_seq;
                            let event_log = Arc::clone(&entry.event_log);
                            event_log.synchronize_tail(&mut entry.session.last_seq);
                            entry.input = None;
                            entry.conversation = None;
                            if let Some(runtime) = entry.runtime.clone() {
                                if !runtimes.iter().any(|known| known.0 == runtime.0) {
                                    runtimes.push(runtime);
                                }
                            }
                            true
                        });
                    (installed, runtimes)
                };
                let task = tokio::spawn(async move {
                    let _lifecycle = lifecycle;
                    let result = service
                        .teardown_compatibility_lifecycle_runtimes(
                            live_adapter.as_ref(),
                            &committed,
                            &local_runtimes,
                            "session stop",
                        )
                        .await
                        .and_then(|_| {
                            installed
                                .then_some(live_owner)
                                .ok_or(CoreError::SessionNotFound)
                        });
                    if result.is_ok() {
                        let mut entries = service.entries.write().await;
                        let is_original = entries
                            .get(&session_id)
                            .is_some_and(|entry| Arc::ptr_eq(&entry.screen, &entry_screen));
                        if is_original {
                            entries.remove(&session_id);
                        }
                        tracing::info!(session_id = %session_id, "session stopped");
                    }
                    result
                });
                (task, ())
            };
            let on_rejected = move |_error| async move {
                rejected_service
                    .synchronize_ordered_live_tail_for(&rejected_id, &rejected_screen)
                    .await;
            };
            let commit = if phase_one_complete {
                self.persist_durably_with(
                    record.clone(),
                    Vec::new(),
                    continuation,
                    on_rejected,
                    drop_undelivered,
                )
            } else {
                append_with_rotation_claim(
                    entry,
                    Some(&record),
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
                        intended.rotation_claim = None;
                    },
                    continuation,
                    on_rejected,
                    drop_undelivered,
                )
            };
            drop(entries);
            let task = commit.committed().await.map_err(store_failed)?;
            return Self::await_compatibility_lifecycle_task(task, "session stop").await;
        }
        drop(entries);

        // Match termination's two-phase boundary for a post-restart session.
        let mut resting = record.clone();
        resting.session.status = SessionStatus::Idle;
        resting.session.stop_reason = Some(StopReason::UserTerminated);
        resting.session.recovery_state = RecoveryState::Idle;
        resting.rotation_claim = None;
        let service = Arc::clone(self);
        let session_id = id.clone();
        let events = if phase_one_complete {
            Vec::new()
        } else {
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
            }]
        };
        let task = self
            .persist_durably_with(
                resting,
                events,
                move |committed| async move {
                    let task = tokio::spawn(async move {
                        let _lifecycle = lifecycle;
                        let result = service
                            .teardown_compatibility_lifecycle_runtimes(
                                adapter.as_ref(),
                                &committed,
                                &[],
                                "session stop",
                            )
                            .await
                            .map(|_| owner);
                        if result.is_ok() {
                            tracing::info!(session_id = %session_id, "session stopped");
                        }
                        result
                    });
                    (task, ())
                },
                ignore_rejected,
                drop_undelivered,
            )
            .committed()
            .await
            .map_err(store_failed)?;
        Self::await_compatibility_lifecycle_task(task, "session stop").await
    }

    /// Stop a coordinated runtime by retiring exact authority before deletion.
    async fn rest_session_coordinated(
        self: &Arc<Self>,
        record: SessionRecord,
        mut lease: crate::coordination::SessionLease,
        lifecycle: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<SubjectId, CoreError> {
        let id = record.session.id.clone();
        let owner = record.session.owner.clone();
        let adapter = self.adapters.get(&record.session.kernel).cloned();
        if self.entries.read().await.contains_key(&id) {
            self.capture_transcript(&id).await;
        }
        let input_fence = self.input_gates.gate(&id).write_owned().await;

        let retired = if record.runtime_id.is_some() {
            match self.retire_current_runtime(&mut lease).await {
                Ok(retired) => Some(retired),
                Err(error) => {
                    if !self.confirm_retired_lifecycle_authority(&mut lease).await {
                        // The retirement may have committed despite both lost
                        // responses. Never reopen the input fence around a
                        // locally cached sender until durable authority can be
                        // read and adopted under this exact lease.
                        self.entries.write().await.remove(&id);
                        drop(input_fence);
                        let _ = self.store.release_session_lease(&lease).await;
                        drop(lifecycle);
                        return Err(store_failed(error));
                    }
                    tracing::warn!(
                        session_id = %id,
                        %error,
                        "runtime retirement was confirmed after an unknown store result"
                    );
                    None
                }
            }
        } else {
            None
        };
        let mut resting = match self.store.get(&id).await {
            Ok(Some(resting)) if resting.runtime_id.is_none() => resting,
            Ok(Some(_)) => {
                self.entries.write().await.remove(&id);
                drop(input_fence);
                let _ = self.store.release_session_lease(&lease).await;
                drop(lifecycle);
                return Err(CoreError::RuntimeUnavailable(
                    "runtime authority changed while the session was stopping".into(),
                ));
            }
            Ok(None) => {
                self.entries.write().await.remove(&id);
                drop(input_fence);
                let _ = self.store.release_session_lease(&lease).await;
                drop(lifecycle);
                return Err(CoreError::SessionNotFound);
            }
            Err(error) => {
                self.entries.write().await.remove(&id);
                drop(input_fence);
                let _ = self.store.release_session_lease(&lease).await;
                drop(lifecycle);
                return Err(store_failed(error));
            }
        };
        resting.session.status = SessionStatus::Idle;
        resting.session.stop_reason = Some(StopReason::UserTerminated);
        resting.session.recovery_state = RecoveryState::Idle;
        resting.rotation_claim = None;
        let persisted = self
            .persist_durably(
                &mut resting,
                vec![Event {
                    session_id: id.clone(),
                    seq: 0,
                    kind: EventKind::StatusChanged {
                        status: SessionStatus::Idle,
                        stop_reason: Some(StopReason::UserTerminated),
                        detail: None,
                        capability: None,
                    },
                    appended_at: String::new(),
                }],
            )
            .await;
        if let Err(error) = persisted {
            self.entries.write().await.remove(&id);
            drop(input_fence);
            let _ = self.store.release_session_lease(&lease).await;
            drop(lifecycle);
            return Err(store_failed(error));
        }
        let boundary = crate::coordination::SessionBoundary::for_record(
            &resting,
            lease.boundary.authority.generation,
        );
        if let Err(error) = self.advance_runtime_lease(&mut lease, &boundary).await {
            self.entries.write().await.remove(&id);
            drop(input_fence);
            let _ = self.store.release_session_lease(&lease).await;
            drop(lifecycle);
            return Err(store_failed(error));
        }
        self.entries.write().await.remove(&id);
        drop(input_fence);

        if let (Some(adapter), Some(retired)) = (adapter, retired) {
            if let Err(error) = self
                .reclaim_runtime_with_lease(&adapter, &lease, &retired.cleanup)
                .await
            {
                tracing::warn!(
                    session_id = %id,
                    runtime = %retired.cleanup.runtime_id,
                    %error,
                    "stopped runtime cleanup remains durable for retry"
                );
            }
        }
        if let Err(error) = self.store.release_session_lease(&lease).await {
            tracing::debug!(session_id = %id, %error, "stop lease ended before explicit release");
        }
        drop(lifecycle);
        tracing::info!(session_id = %id, "session stopped");
        Ok(owner)
    }

    /// Apply an operator abuse-control transition without touching the shared
    /// host runtime borrowed by this thread.
    async fn set_chat_thread_lifecycle(
        &self,
        observed: SessionRecord,
        status: SessionStatus,
        recovery_state: RecoveryState,
        detail: &'static str,
    ) -> Result<SubjectId, CoreError> {
        let id = observed.session.id.clone();
        let owner = observed.session.owner.clone();
        let _input_fence = self.input_gates.gate(&id).write_owned().await;
        let ordering = self.conversation_order_gates.gate(&id);
        let _ordered = ordering.lock().await;
        let mut current = self
            .store
            .get(&id)
            .await
            .map_err(store_failed)?
            .ok_or(CoreError::SessionNotFound)?;
        if current.chat_origin != observed.chat_origin || current.runtime_id != observed.runtime_id
        {
            return Err(CoreError::RuntimeUnavailable(
                "the chat thread changed while its lifecycle was being updated".into(),
            ));
        }
        if current.session.status.is_terminal() {
            if status == SessionStatus::Terminated {
                self.entries.write().await.remove(&id);
                return Ok(owner);
            }
            return Err(CoreError::SessionTerminated);
        }
        current.session.status = status;
        current.session.stop_reason = Some(StopReason::UserTerminated);
        current.session.recovery_state = recovery_state;
        self.persist_durably(
            &mut current,
            vec![Event {
                session_id: id.clone(),
                seq: 0,
                kind: EventKind::StatusChanged {
                    status,
                    stop_reason: Some(StopReason::UserTerminated),
                    detail: Some(detail.into()),
                    capability: None,
                },
                appended_at: String::new(),
            }],
        )
        .await
        .map_err(store_failed)?;
        self.entries.write().await.remove(&id);
        Ok(owner)
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
        let initial = self.record(id).await?;
        if !initial.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if initial.chat_origin.is_some() {
            return self.delete_chat_thread(initial).await;
        }
        // Idempotent termination also repairs a terminal row whose authority or
        // exact cleanup debt survived an earlier process. Delete must not skip
        // that repair and then erase the only row capable of safe reclamation.
        self.terminate_session(caller, id).await?;
        let cleanup_record = self
            .finish_terminal_skill_pack_cleanup_with_authority(
                id,
                crate::coordination::SessionOperation::Lifecycle,
            )
            .await
            .map_err(store_failed)?;
        self.entries.write().await.remove(id);
        loop {
            match self.delete_durably(id).await {
                Ok(()) => break,
                Err(StoreError::SessionCleanupPending { .. })
                    if self.starting.read().await.contains(id) =>
                {
                    // A bring-up that began before termination can briefly
                    // record its handle before its installation boundary sees
                    // the terminal state and tears that exact runtime down.
                    // Keep the session row as cleanup authority until the
                    // locally tracked task finishes; unrelated durable debt is
                    // returned to the caller instead of being polled away.
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Err(error) => return Err(store_failed(error)),
            }
        }
        // The events go with the session, so the position kept for it is a
        // number held for something nothing can read. Only here: a session
        // that merely stopped keeps its position, because it can be revived
        // and its log continues rather than starting again.
        self.sequences.forget(id);

        // The transcript goes with the session. Leaving it behind would keep a
        // record of work its owner asked to be removed, and orphan a blob
        // nothing references.
        if let (Some(store), Some(reference)) =
            (&self.context, cleanup_record.context_ref.as_deref())
        {
            if let Err(e) = store.delete(reference).await {
                tracing::error!(session_id = %id, error = %e, "cannot remove stored context");
            }
        }
        // The handover object has a deterministic per-session key and may be
        // retained after its descriptor was cleared by successful H2
        // publication. Delete by session unconditionally rather than relying
        // on the pre-delete record still carrying a reference.
        if let Some(store) = &self.handovers {
            if let Err(error) = store.delete_for(id).await {
                tracing::error!(session_id = %id, %error, "cannot remove stored handover snapshot");
            }
        }
        tracing::info!(session_id = %id, "session deleted");
        Ok(cleanup_record.context_ref)
    }

    async fn delete_chat_thread(
        self: &Arc<Self>,
        observed: SessionRecord,
    ) -> Result<Option<String>, CoreError> {
        let id = observed.session.id.clone();
        let lifecycle = self.lock_lifecycle(&id).await;
        let input_fence = self.input_gates.gate(&id).write_owned().await;
        let ordering = self.conversation_order_gates.gate(&id);
        let ordered = ordering.lock().await;
        let current = self
            .store
            .get(&id)
            .await
            .map_err(store_failed)?
            .ok_or(CoreError::SessionNotFound)?;
        let origin = current
            .chat_origin
            .clone()
            .ok_or_else(|| CoreError::chat_lifecycle_managed("delete transition"))?;
        if current.session.owner != observed.session.owner {
            return Err(CoreError::Forbidden);
        }
        let host_store = self.chat_hosts.clone().ok_or({
            CoreError::ChatUnavailable(crate::ChatUnavailableCause::HostStoreUnavailable)
        })?;
        let owner_guard = self.chat_host_guards.lock(&current.session.owner).await;
        let mut host = host_store
            .get(&origin.host_id)
            .await
            .map_err(|_| {
                CoreError::ChatUnavailable(crate::ChatUnavailableCause::HostStoreUnavailable)
            })?
            .ok_or(CoreError::ChatUnavailable(
                crate::ChatUnavailableCause::Retired,
            ))?;
        let slot_removed = host
            .threads
            .iter()
            .find(|slot| slot.session_id == id)
            .map(|slot| slot.removed)
            .ok_or_else(|| {
                CoreError::RuntimeUnavailable(
                    "the chat host no longer contains this thread allocation".into(),
                )
            })?;
        let has_other_retained_threads = host
            .threads
            .iter()
            .any(|slot| slot.session_id != id && !slot.removed);
        if !slot_removed {
            match host.status {
                crate::ChatHostStatus::Active | crate::ChatHostStatus::Pausing => {
                    let runtime_id = host.runtime_id.clone().ok_or({
                        CoreError::ChatUnavailable(crate::ChatUnavailableCause::Provisioning)
                    })?;
                    if current.runtime_id.as_deref() != Some(runtime_id.as_str()) {
                        return Err(CoreError::RuntimeUnavailable(
                            "the chat thread does not name its host's current runtime".into(),
                        ));
                    }
                    if has_other_retained_threads {
                        let adapter = self.adapters.get(&host.kernel).ok_or_else(|| {
                            CoreError::ChatUnavailable(
                                crate::ChatUnavailableCause::CapabilityUnsupported(
                                    "configured chat kernel".into(),
                                ),
                            )
                        })?;
                        adapter
                            .remove_workspace_tree(
                                &RuntimeHandle(runtime_id),
                                &origin.worktree_name,
                            )
                            .await
                            .map_err(|_| {
                                CoreError::ChatUnavailable(
                                    crate::ChatUnavailableCause::ProvisioningFailed {
                                        phase: crate::ChatProvisioningPhase::Cleanup,
                                        command_index: None,
                                        retryable: true,
                                    },
                                )
                            })?;
                    }
                }
                crate::ChatHostStatus::Paused => {}
                crate::ChatHostStatus::Reviving => {
                    return Err(CoreError::ChatUnavailable(
                        crate::ChatUnavailableCause::RevivePending,
                    ));
                }
                crate::ChatHostStatus::Provisioning => {
                    return Err(CoreError::ChatUnavailable(
                        crate::ChatUnavailableCause::Provisioning,
                    ));
                }
                crate::ChatHostStatus::Retired => {}
            }
        }
        loop {
            let slot = host
                .threads
                .iter_mut()
                .find(|slot| slot.session_id == id)
                .ok_or_else(|| {
                    CoreError::RuntimeUnavailable(
                        "the chat host no longer contains this thread allocation".into(),
                    )
                })?;
            slot.removed = true;
            if let Some(mut checkpoint) = host.checkpoint.take() {
                checkpoint
                    .conversations
                    .retain(|conversation| conversation.session_id != id);
                if !checkpoint.conversations.is_empty() {
                    checkpoint.reseal().map_err(CoreError::InvalidRequest)?;
                    host.checkpoint = Some(checkpoint);
                }
            }
            host.updated_at = crate::clock::now_rfc3339();
            match host_store
                .update_if_revision(host.revision, &host)
                .await
                .map_err(|_| {
                    CoreError::ChatUnavailable(crate::ChatUnavailableCause::HostStoreUnavailable)
                })? {
                Some(committed) => {
                    host = committed;
                    break;
                }
                None => {
                    host = host_store
                        .get(&origin.host_id)
                        .await
                        .map_err(|_| {
                            CoreError::ChatUnavailable(
                                crate::ChatUnavailableCause::HostStoreUnavailable,
                            )
                        })?
                        .ok_or({
                            CoreError::ChatUnavailable(crate::ChatUnavailableCause::Retired)
                        })?;
                }
            }
        }
        self.entries.write().await.remove(&id);
        let context_ref = current.context_ref;
        drop(owner_guard);
        self.retire_chat_host_if_empty(&host.id).await?;
        self.delete_durably(&id).await.map_err(store_failed)?;
        self.sequences.forget(&id);
        drop(ordered);
        drop(input_fence);
        drop(lifecycle);
        Ok(context_ref)
    }
}
