//! Carrying a session onto another runtime without interrupting it.
//!
//! Two things ask for this and both are the same act. The runtime image moves
//! on and sessions started on the old one have to follow it; or an owner asks
//! to be shown their session as a conversation rather than as a screen, which
//! means the same agent under a different front-end. Either way the session is
//! captured, its sandbox is released, a replacement is started, and the agent
//! resumes from the capture — so this is written once and told what changes.
//!
//! Upgrading a sandbox in place is not an option: the image is the sandbox.
//! So the session is moved instead, and its identifier, its history, its
//! configuration and its place in its owner's list are all unchanged, because
//! none of those were ever properties of the sandbox.
//!
//! Two things make this honest rather than merely invisible.
//!
//! The snapshot has a moment. Anything the agent did after it is not carried
//! across, so the moment is recorded and shown, and the owner can tell what
//! was included rather than guessing why something is missing.
//!
//! The session refuses input while it happens. Accepting keystrokes bound for
//! a sandbox that is being torn down would lose them silently, which is worse
//! than briefly declining them and saying so.
//!
//! The snapshot itself is opaque here. It is captured by an adapter, stored by
//! the core and handed back to an adapter, and nothing along that path reads
//! it — only the agent that produced one knows how. That is what lets a new
//! kernel be added without teaching this file anything about it.

use std::sync::Arc;

use layer3_adapter::RuntimeHandle;

use crate::authz::Caller;
use crate::capabilities::SessionCapabilities;
use crate::clock::now_rfc3339;
use crate::coordination::{SessionBoundary, SessionLease, SessionOperation};
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::options::SessionMode;
use crate::service::SessionService;
use crate::session::{Session, SessionSpec, SessionStatus};
use crate::store::{
    HandoverSnapshot, PendingHandover, PendingHandoverTarget, SessionRecord, StoreError,
};

/// What a handover changes about the session it carries.
///
/// The only difference between the two, and it is deliberately small: a
/// handover is a handover, and forking the path would give the rarer one a
/// second set of failure cases nobody exercises.
enum Handover {
    /// Onto the runtime version this deployment expects.
    Version(String),
    /// Onto the other plane, because the session's owner asked.
    Plane {
        mode: SessionMode,
        /// What the session holds afterwards, derived before anything is torn
        /// down so the move cannot get halfway and find out it is impossible.
        capabilities: SessionCapabilities,
    },
}

/// Durable handover state carried through replacement provisioning.
///
/// The old authority remains live while the candidate is prepared. These
/// exact records let the successful path clear only its own rotation claim and
/// let a pre-switch failure restore only the claim it actually acquired.
pub(crate) struct CoordinatedHandover {
    pub(crate) original: SessionRecord,
    pub(crate) rotating: SessionRecord,
    pub(crate) rotating_boundary: SessionBoundary,
    pub(crate) completion_events: Vec<Event>,
    pub(crate) detached: crate::lease::Detached,
}

impl Handover {
    /// What the owner is told while it happens.
    fn detail(&self) -> &'static str {
        match self {
            Self::Version(_) => {
                "moving this session to an updated runtime; it will come back where it left off"
            }
            Self::Plane { .. } => {
                "moving this session to the other plane; it will come back where it left off"
            }
        }
    }

    /// Durable target carried independently of this process and request.
    fn pending_target(&self, record: &SessionRecord) -> PendingHandoverTarget {
        match self {
            Self::Version(runtime_version) => PendingHandoverTarget::RuntimeVersion {
                from_runtime_version: record.session.runtime_version.clone(),
                runtime_version: runtime_version.clone(),
            },
            Self::Plane { mode, capabilities } => PendingHandoverTarget::Plane {
                from_mode: record.session.options.mode,
                mode: *mode,
                capabilities: *capabilities,
            },
        }
    }
}

impl SessionService {
    /// Whether this session is running something other than what it should be.
    ///
    /// A session with no recorded version counts as behind: it predates
    /// version tracking, so nothing can vouch for what it is running, and
    /// leaving it there forever is the one outcome that is certainly wrong.
    pub(crate) fn needs_rotation(&self, record: &SessionRecord) -> bool {
        let Some(wanted) = self.runtime_version.as_deref() else {
            // No expected version configured, so nothing to be behind.
            return false;
        };
        if !record.session.capabilities.snapshot {
            // Asked statically rather than found out by trying. A session whose
            // runtime cannot hand its conversation over would fail the capture
            // below on every sweep, forever, and each failure would be logged
            // as though something had gone wrong — when the truthful reading is
            // that this session was never movable and its owner is better
            // served by it simply continuing to work.
            return false;
        }
        // An idle approval wait keeps its runtime, but it is deliberately not
        // moved while the old conversation is blocked on a runtime-local
        // request. Resolving the final approval returns it to `Running`, making
        // it eligible on the next sweep.
        record.session.status.wants_runtime()
            && !record.is_effectively_rotating()
            && record.runtime_id.is_some()
            && record.session.runtime_version.as_deref() != Some(wanted)
    }

    /// Move a session onto the current runtime version.
    ///
    /// The sweep's caller, and the reason this module exists. Reports only
    /// whether the move started: nothing is waiting on the answer, and a
    /// session left where it was is a state the next sweep reconsiders.
    pub(crate) async fn rotate(self: &Arc<Self>, record: SessionRecord) -> bool {
        let id = record.session.id.clone();
        let Some(wanted) = self.runtime_version.clone() else {
            return false;
        };
        tracing::info!(
            session_id = %id,
            from = record.session.runtime_version.as_deref().unwrap_or("<unknown>"),
            to = %wanted,
            "moving a session onto a newer runtime"
        );
        self.carry_over(record, Handover::Version(wanted))
            .await
            .is_ok()
    }

    /// Move a live session onto the other plane, because its owner asked.
    ///
    /// The plane is fixed for a session's life in the sense that matters — it
    /// cannot drift, and nothing changes it quietly — but it is not beyond
    /// reach: this is an explicit, owner-initiated act that leaves the session
    /// holding exactly one plane afterwards and records that it happened.
    ///
    /// It is a rotation. The same capture, the same replacement, the same
    /// resume by identifier, and the same `rotating` window in which input is
    /// refused rather than lost; the only difference is that the replacement
    /// is started under the other front-end instead of a newer image.
    ///
    /// Every refusal comes before anything is torn down, and a session that is
    /// refused keeps running exactly as it was. Losing a conversation to a
    /// convenience is the defect `CLAUDE.md` §5 exists to prevent.
    pub async fn change_plane(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        mode: SessionMode,
    ) -> Result<Session, CoreError> {
        let record = self.record(id).await?;
        caller.require_own_control_of(&record.session.owner)?;
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if record.is_effectively_rotating() {
            // Already mid-handover. Starting a second one would capture a
            // sandbox being torn down and resume from whichever answered last.
            return Err(CoreError::InvalidState);
        }
        // A blocked approval owns a runtime too, but cannot change planes while
        // its answer is addressed to a request on the current conversation.
        if !record.session.status.wants_runtime() || record.runtime_id.is_none() {
            // Nothing to carry. A stopped session is started on whichever
            // plane its profile names when it is revived, and moving one that
            // is not running would mean rotating a sandbox that does not exist.
            return Err(CoreError::InvalidState);
        }
        if record.session.options.mode == mode {
            // Already there. Not an error and not a rotation: charging a
            // session a handover to arrive where it is would be a cost with
            // nothing on the other side of it.
            return Ok(record.session);
        }

        let Some(adapter) = self.adapters.get(&record.session.kernel).cloned() else {
            return Err(CoreError::RuntimeUnavailable(format!(
                "this deployment no longer runs {}",
                record.session.kernel.as_str()
            )));
        };
        // Named before anything is provisioned, exactly as creation does it:
        // a kernel that does not advertise the plane is refused here, with the
        // plane's own name, rather than after its sandbox is gone.
        let capabilities = SessionCapabilities::granted(mode, adapter.capabilities())?;

        if !record.session.capabilities.snapshot {
            // The fail-safe. Both front-ends of the one kernel that serves
            // both planes read each other's conversations — established
            // against the pinned version on #76 — but a kernel that cannot
            // hand its conversation over at all cannot be moved anywhere, and
            // attempting it would trade the conversation for the surface.
            return Err(CoreError::CapabilityUnsupported(
                crate::capabilities::Capability::Snapshot.name().to_string(),
            ));
        }

        tracing::info!(
            session_id = %id,
            from = ?record.session.options.mode,
            to = ?mode,
            "moving a session onto the other plane"
        );
        self.carry_over(record, Handover::Plane { mode, capabilities })
            .await
    }

    /// The handover itself, whatever asked for it.
    ///
    /// Ordered so that the failure cases are survivable. The snapshot is taken
    /// before anything is torn down, so a snapshot that fails costs nothing;
    /// the new runtime is started before the session is declared usable, so a
    /// start that fails leaves the session recoverable by the same path that
    /// recovers any other lost runtime.
    async fn carry_over(
        self: &Arc<Self>,
        record: SessionRecord,
        change: Handover,
    ) -> Result<Session, CoreError> {
        let id = record.session.id.clone();
        if !self.adapters.contains_key(&record.session.kernel) {
            return Err(CoreError::RuntimeUnavailable(format!(
                "this deployment no longer runs {}",
                record.session.kernel.as_str()
            )));
        }
        if record.runtime_id.is_none() {
            return Err(CoreError::InvalidState);
        }
        if self.rotation_refused.read().await.contains(&id) {
            // This runtime has already said it cannot hand its conversation
            // over. Asking again every few minutes would achieve nothing and
            // would keep the question in the log forever.
            return Err(CoreError::CapabilityUnsupported(
                crate::capabilities::Capability::Snapshot.name().to_string(),
            ));
        }
        if !self.begin_start(&id).await {
            // Something else already has this session in hand.
            return Err(CoreError::InvalidState);
        }

        // From the first local claim onward this work belongs to the service,
        // not to the HTTP request that happened to trigger it. A disconnected
        // caller drops only this receiver; the handover still records or rolls
        // back its durable claim and releases `starting` in its own task.
        let (finished, result) = tokio::sync::oneshot::channel();
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = service.carry_over_claimed(record, change).await;
            let _ = finished.send(outcome);
        });
        result.await.map_err(|_| {
            CoreError::RuntimeUnavailable("this session's handover task stopped".into())
        })?
    }

    /// A service-owned handover after `starting` has been claimed.
    async fn carry_over_claimed(
        self: &Arc<Self>,
        record: SessionRecord,
        change: Handover,
    ) -> Result<Session, CoreError> {
        let id = record.session.id.clone();
        let Some(adapter) = self.adapters.get(&record.session.kernel).cloned() else {
            self.finish_start(&id).await;
            return Err(CoreError::RuntimeUnavailable(format!(
                "this deployment no longer runs {}",
                record.session.kernel.as_str()
            )));
        };
        let Some(old) = record.runtime_id.clone().map(RuntimeHandle) else {
            self.finish_start(&id).await;
            return Err(CoreError::InvalidState);
        };

        // Refresh before acquiring cross-replica provider authority. The lease
        // must cover every custody read or write as well as snapshot/provider
        // work; otherwise a remote lifecycle operation can delete the object
        // between preflight and H2 publication.
        let record = match self.store.get(&id).await {
            Ok(Some(current))
                if current.session.wants_runtime()
                    && !current.is_effectively_rotating()
                    && current.session.kernel == record.session.kernel
                    && current.runtime_id.as_deref() == Some(old.0.as_str()) =>
            {
                current
            }
            Ok(_) => {
                self.finish_start(&id).await;
                return Err(CoreError::InvalidState);
            }
            Err(error) => {
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(format!(
                    "this session could not be checked before handover authority was acquired, so its runtime has not been moved: {error}"
                )));
            }
        };

        let lease = match self
            .acquire_runtime_lease_at(&record, SessionOperation::Handover)
            .await
        {
            Ok(lease) => lease,
            Err(error) => {
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(format!(
                    "this session's handover authority could not be acquired: {error}"
                )));
            }
        };
        if let Some(lease) = lease {
            if let Err(error) = self.renew_runtime_lease(Some(&lease)).await {
                let _ = self.store.release_session_lease(&lease).await;
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(format!(
                    "the handover lease expired before skill preparation: {error}"
                )));
            }
            let prepared_skills = match self
                .prepare_skill_packs_for_handover(&record.session, Some(&lease))
                .await
            {
                Ok(prepared) => prepared,
                Err(failure) => {
                    tracing::warn!(
                        session_id = %id,
                        reason = %failure.reason,
                        "cannot reproduce this session's frozen skills; leaving its runtime in place"
                    );
                    let _ = self.store.release_session_lease(&lease).await;
                    self.finish_start(&id).await;
                    return Err(CoreError::RuntimeUnavailable(format!(
                        "this session's frozen skills could not be prepared, so its runtime has not been moved: {}",
                        failure.reason
                    )));
                }
            };
            if let Err(error) = self.renew_runtime_lease(Some(&lease)).await {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "the handover lease expired while exact skill bytes were being prepared"
                );
                let _ = self.store.release_session_lease(&lease).await;
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(
                    "this session's skill preflight outlived its handover authority; its runtime has not been moved"
                        .into(),
                ));
            }
            return self
                .carry_over_coordinated(record, change, adapter, old, lease, prepared_skills)
                .await;
        }

        let prepared_skills = match self
            .prepare_skill_packs_for_handover(&record.session, None)
            .await
        {
            Ok(prepared) => prepared,
            Err(failure) => {
                tracing::warn!(
                    session_id = %id,
                    reason = %failure.reason,
                    "cannot reproduce this session's frozen skills; leaving its runtime in place"
                );
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(format!(
                    "this session's frozen skills could not be prepared, so its runtime has not been moved: {}",
                    failure.reason
                )));
            }
        };

        let Some(handovers) = self.handovers.as_ref().cloned() else {
            self.finish_start(&id).await;
            return Err(CoreError::RuntimeUnavailable(
                "this deployment cannot durably store a runtime handover snapshot".into(),
            ));
        };

        // Drain every operation already admitted to H1, then hold new input
        // behind this boundary while snapshot storage and the durable
        // descriptor are published. A failed capture, upload, or record write
        // releases the fence with H1 and its public status untouched.
        let input_fence = self.input_gates.gate(&id).write_owned().await;
        let snapshot_at = now_rfc3339();
        let snapshot = match adapter.snapshot(&old).await {
            Ok(bytes) => bytes,
            Err(e) => {
                // The session cannot be carried across, so it stays exactly
                // where it is. A stale image is bad; losing the conversation
                // is worse. Remembered so this is not retried every few
                // minutes against a runtime that has already said it cannot.
                tracing::warn!(session_id = %id, error = %e, "cannot move this session; leaving it in place");
                self.rotation_refused.write().await.insert(id.clone());
                drop(input_fence);
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(format!(
                    "this session's conversation could not be captured, so it has not been moved: {e}"
                )));
            }
        };
        let claim = format!("rotn_{}", uuid::Uuid::new_v4().simple());
        let target = change.pending_target(&record);
        let snapshot_ref = match handovers
            .put(
                &id,
                HandoverSnapshot {
                    claim: claim.clone(),
                    snapshot_at: snapshot_at.clone(),
                    restore: snapshot,
                },
            )
            .await
        {
            Ok(reference) => reference,
            Err(error) => {
                tracing::warn!(session_id = %id, %error, "cannot stage the handover snapshot; leaving the runtime in place");
                drop(input_fence);
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(
                    "this session's conversation could not be stored, so it has not been moved"
                        .into(),
                ));
            }
        };
        let pending = PendingHandover {
            claim,
            snapshot_ref,
            snapshot_at,
            rotations_from: record.session.rotations,
            rotations_to: record.session.rotations.saturating_add(1),
            target,
        };

        // The claim and recovery descriptor become durable in one write while
        // all post-snapshot input remains held behind the same fence.
        let marked = self
            .mark_rotating(&record, &change, Some(pending.clone()), 0, &prepared_skills)
            .await;
        drop(input_fence);
        let Some(rotating) = marked else {
            self.finish_start(&id).await;
            return Err(CoreError::RuntimeUnavailable(
                "this session's handover could not be recorded, so it has not been moved".into(),
            ));
        };
        self.rotations_in_progress.write().await.insert(id.clone());

        // Object storage has no cross-store transaction or conditional write
        // tied to the session row. Compatibility mode is the documented
        // single-writer fallback; even there, require a claim-matching readback
        // before the irreversible provider operation. Coordinated stores use
        // the lease-aware path above instead.
        let restore = match self.load_pending_handover_snapshot(&rotating).await {
            Ok(restore) => restore,
            Err(error) => {
                tracing::warn!(session_id = %id, %error, "the staged handover snapshot cannot be verified; leaving H1 in place");
                let _ = self.restore_after_failed_move(&record, &rotating).await;
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(
                    "this session's stored handover could not be verified, so it has not been moved"
                        .into(),
                ));
            }
        };

        // Credential delivery uses this same legacy lifecycle gate from exact
        // runtime selection through durable promotion and entry evidence. The
        // durable coordination path above uses its Interact/Handover lease
        // instead; this branch is reached only when the store has no such lease.
        let lifecycle = self.lock_lifecycle(&id).await;
        let claimed = match self.store.get(&id).await {
            Ok(Some(current))
                if current.session.status == SessionStatus::Rotating
                    && current.runtime_id == rotating.runtime_id
                    && current.rotation_claim == rotating.rotation_claim
                    && current.pending_handover == rotating.pending_handover =>
            {
                current
            }
            Ok(_) => {
                drop(lifecycle);
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(
                    "this session's handover authority changed before H1 teardown".into(),
                ));
            }
            Err(error) => {
                tracing::warn!(session_id = %id, %error, "cannot verify handover authority before H1 teardown");
                drop(lifecycle);
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(
                    "this session's handover authority could not be verified".into(),
                ));
            }
        };
        // Fence every local channel for this exact H1 before touching the
        // provider. The durable record still names H1, so a failed termination
        // remains retryable and reconciliation can readopt it truthfully.
        let old_entry = {
            let mut entries = self.entries.write().await;
            let exact_old = entries.get(&id).and_then(|entry| entry.runtime.as_ref()) == Some(&old);
            exact_old.then(|| entries.remove(&id)).flatten()
        };
        if let Some(entry) = old_entry {
            if let Some(attached) = entry.attached.as_ref() {
                let _ = attached.supersede.send(match &change {
                    Handover::Version(_) => crate::lease::Detached::RuntimeReplaced,
                    Handover::Plane { .. } => crate::lease::Detached::PlaneChanged,
                });
            }
        }
        let cleared = match self
            .teardown_compatibility_runtime(&adapter, &claimed, "runtime handover")
            .await
        {
            Ok(cleared) => cleared,
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "could not release the old runtime; leaving the handover fenced"
                );
                drop(lifecycle);
                self.finish_start(&id).await;
                return Err(error);
            }
        };

        let moved = match self.publish_pending_handover_target(cleared).await {
            Ok(moved) => moved,
            Err(error) => {
                tracing::error!(session_id = %id, %error, "cannot publish the move after clearing the old runtime");
                drop(lifecycle);
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(
                    "this session's move could not be recorded after its old runtime was cleared"
                        .into(),
                ));
            }
        };
        drop(lifecycle);

        let geometry = moved.session.geometry;
        let spec = SessionSpec {
            geometry: Some(geometry),
            options: moved.session.options.clone(),
            environment_profile: moved.session.environment_profile.clone(),
            caller_credential: None,
            runtime_credential: None,
            requested_mode: None,
            quoted_rate_card: None,
        };
        let service = Arc::clone(self);
        let session = moved.session.clone();
        let rotation_claim = moved.rotation_claim.clone();
        tokio::spawn(async move {
            service
                .bring_up(
                    session,
                    spec,
                    adapter,
                    geometry,
                    restore,
                    crate::provision_support::BringUpContext::compatibility(
                        crate::provision_failure::Abandonment::Keep,
                        rotation_claim,
                    )
                    .with_prepared_skills(prepared_skills),
                )
                .await;
        });
        Ok(moved.session)
    }

    /// Load only the claim-bound object named by this durable descriptor.
    pub(crate) async fn load_pending_handover_snapshot(
        &self,
        record: &SessionRecord,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let envelope = self.read_pending_handover_snapshot(record).await?;
        let pending = record.pending_handover.as_ref().ok_or_else(|| {
            StoreError::Malformed("a compatibility handover has no durable descriptor".into())
        })?;
        if envelope.snapshot_at != pending.snapshot_at {
            return Err(StoreError::Malformed(
                "the stored handover timestamp does not match its descriptor".into(),
            ));
        }
        Ok(envelope.restore)
    }

    async fn read_pending_handover_snapshot(
        &self,
        record: &SessionRecord,
    ) -> Result<HandoverSnapshot, StoreError> {
        let pending = record.pending_handover.as_ref().ok_or_else(|| {
            StoreError::Malformed("a compatibility handover has no durable descriptor".into())
        })?;
        if pending.claim.is_empty()
            || pending.snapshot_ref.is_empty()
            || pending.snapshot_at.is_empty()
        {
            return Err(StoreError::Malformed(
                "a compatibility handover descriptor is incomplete".into(),
            ));
        }
        match record.rotation_claim.as_deref() {
            Some(claim) if claim == pending.claim => {}
            // Stop intentionally retires the active claim while preserving an
            // unconsumed descriptor. Revive may verify those bytes before it
            // atomically reinstates the descriptor's claim.
            None if record.session.status == SessionStatus::Idle => {}
            _ => {
                return Err(StoreError::Malformed(
                    "the handover descriptor does not own this rotation claim".into(),
                ))
            }
        }
        let handovers = self.handovers.as_ref().ok_or_else(|| {
            StoreError::Unavailable("handover object storage is not configured".into())
        })?;
        let envelope = handovers
            .get_for(&record.session.id, &pending.snapshot_ref)
            .await?
            .ok_or_else(|| {
                StoreError::Unavailable("the handover snapshot object is missing".into())
            })?;
        if envelope.claim != pending.claim {
            return Err(StoreError::Malformed(
                "the stored handover snapshot belongs to another claim".into(),
            ));
        }
        if envelope.snapshot_at.is_empty() {
            return Err(StoreError::Malformed(
                "the stored handover snapshot has no capture time".into(),
            ));
        }
        Ok(envelope)
    }

    /// Publish one pending target exactly once after H1 is durably absent.
    ///
    /// A fresh process may observe a target commit whose response was lost.
    /// Exact target semantics together with `rotated_at == snapshot_at` are the
    /// durable receipt; any partial third state is refused rather than charged
    /// or logged as a second rotation.
    pub(crate) async fn publish_pending_handover_target(
        &self,
        record: SessionRecord,
    ) -> Result<SessionRecord, StoreError> {
        let pending = record.pending_handover.clone().ok_or_else(|| {
            StoreError::Malformed("a compatibility handover has no durable target".into())
        })?;
        if record.rotation_claim.as_deref() != Some(pending.claim.as_str())
            || record.runtime_id.is_some()
        {
            return Err(StoreError::Malformed(
                "the pending handover does not own the cleared H1 boundary".into(),
            ));
        }

        let (target_applied, source_matches) = match &pending.target {
            PendingHandoverTarget::RuntimeVersion {
                from_runtime_version,
                runtime_version,
            } => (
                record.session.runtime_version.as_deref() == Some(runtime_version.as_str()),
                &record.session.runtime_version == from_runtime_version,
            ),
            PendingHandoverTarget::Plane {
                from_mode,
                mode,
                capabilities,
            } => (
                record.session.options.mode == *mode
                    && record.session.capabilities == *capabilities,
                record.session.options.mode == *from_mode,
            ),
        };
        let target_receipt = target_applied
            && record.session.rotated_at.as_deref() == Some(pending.snapshot_at.as_str())
            && record.session.rotations == pending.rotations_to;
        if target_receipt {
            return Ok(record);
        }
        if target_applied
            || !source_matches
            || record.session.rotations != pending.rotations_from
            || record.session.status != SessionStatus::Rotating
        {
            return Err(StoreError::Malformed(
                "the pending handover is neither at its source nor its exact target receipt".into(),
            ));
        }

        let mut moved = record.clone();
        let events = match &pending.target {
            PendingHandoverTarget::RuntimeVersion {
                runtime_version, ..
            } => {
                moved.session.runtime_version = Some(runtime_version.clone());
                Vec::new()
            }
            PendingHandoverTarget::Plane {
                from_mode,
                mode,
                capabilities,
            } => {
                moved.session.options.mode = *mode;
                moved.session.capabilities = *capabilities;
                vec![Event {
                    session_id: moved.session.id.clone(),
                    seq: 0,
                    kind: EventKind::PlaneChanged {
                        from: *from_mode,
                        to: *mode,
                        snapshot_at: pending.snapshot_at.clone(),
                    },
                    appended_at: String::new(),
                }]
            }
        };
        moved.session.rotations = pending.rotations_to;
        moved.session.rotated_at = Some(pending.snapshot_at);
        self.complete_compatibility_handover_durably(&mut moved, &record, events)
            .await?;
        Ok(moved)
    }

    /// Replace a runtime without surrendering the old authority first.
    ///
    /// The lease is acquired by the caller before the first snapshot. The old
    /// handle and live entry remain authoritative through provider create and
    /// attachment; `start_runtime` alone can switch them after the candidate is
    /// usable. A pre-switch failure can therefore return to this exact record.
    async fn carry_over_coordinated(
        self: &Arc<Self>,
        record: SessionRecord,
        change: Handover,
        adapter: Arc<dyn layer3_adapter::KernelAdapter>,
        old: RuntimeHandle,
        mut lease: SessionLease,
        prepared_skills: crate::provision_support::PreparedSkillPacks,
    ) -> Result<Session, CoreError> {
        let id = record.session.id.clone();
        if let Err(error) = self.renew_runtime_lease(Some(&lease)).await {
            let _ = self.store.release_session_lease(&lease).await;
            self.finish_start(&id).await;
            return Err(CoreError::RuntimeUnavailable(format!(
                "the handover lease expired before capture: {error}"
            )));
        }
        let input_fence = self.input_gates.gate(&id).write_owned().await;
        let taken_at = now_rfc3339();
        let restore = match adapter.snapshot(&old).await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(session_id = %id, %error, "cannot move this session; leaving it in place");
                self.rotation_refused.write().await.insert(id.clone());
                drop(input_fence);
                let _ = self.store.release_session_lease(&lease).await;
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(format!(
                    "this session's conversation could not be captured, so it has not been moved: {error}"
                )));
            }
        };

        let marked = self
            .mark_rotating(
                &record,
                &change,
                None,
                lease.boundary.authority.generation,
                &prepared_skills,
            )
            .await;
        drop(input_fence);
        let Some(rotating) = marked else {
            let _ = self.store.release_session_lease(&lease).await;
            self.finish_start(&id).await;
            return Err(CoreError::RuntimeUnavailable(
                "this session's handover could not be recorded, so it has not been moved".into(),
            ));
        };
        let rotating_boundary =
            SessionBoundary::for_record(&rotating, lease.boundary.authority.generation);
        if let Err(error) = self
            .advance_runtime_lease(&mut lease, &rotating_boundary)
            .await
        {
            tracing::error!(session_id = %id, %error, "cannot advance the handover lease");
            let _ = self.restore_after_failed_move(&record, &rotating).await;
            let _ = self.store.release_session_lease(&lease).await;
            self.finish_start(&id).await;
            return Err(CoreError::RuntimeUnavailable(
                "this session's handover authority changed while it was being recorded".into(),
            ));
        }
        self.rotations_in_progress.write().await.insert(id.clone());

        if let Err(error) = self.renew_runtime_lease(Some(&lease)).await {
            self.rollback_coordinated_handover(&record, &rotating, &mut lease)
                .await;
            return Err(CoreError::RuntimeUnavailable(format!(
                "the handover lease expired before replacement provisioning: {error}"
            )));
        }

        let mut moved = rotating.clone();
        let detached = match &change {
            Handover::Version(wanted) => {
                moved.session.runtime_version = Some(wanted.clone());
                crate::lease::Detached::RuntimeReplaced
            }
            Handover::Plane { mode, capabilities } => {
                moved.session.options.mode = *mode;
                moved.session.capabilities = *capabilities;
                crate::lease::Detached::PlaneChanged
            }
        };
        moved.session.rotations = moved.session.rotations.saturating_add(1);
        moved.session.rotated_at = Some(taken_at.clone());
        let completion_events = if let Handover::Plane { mode, .. } = &change {
            vec![Event {
                session_id: id.clone(),
                seq: 0,
                kind: EventKind::PlaneChanged {
                    from: record.session.options.mode,
                    to: *mode,
                    snapshot_at: taken_at,
                },
                appended_at: String::new(),
            }]
        } else {
            Vec::new()
        };
        let handover = CoordinatedHandover {
            original: record,
            rotating: rotating.clone(),
            rotating_boundary,
            completion_events,
            detached,
        };
        let geometry = moved.session.geometry;
        let spec = SessionSpec {
            geometry: Some(geometry),
            options: moved.session.options.clone(),
            environment_profile: moved.session.environment_profile.clone(),
            caller_credential: None,
            runtime_credential: None,
            requested_mode: None,
            quoted_rate_card: None,
        };
        let service = Arc::clone(self);
        let session = moved.session.clone();
        let rotation_claim = rotating.rotation_claim.clone();
        tokio::spawn(async move {
            service
                .bring_up_with_lease(
                    session,
                    adapter,
                    spec,
                    geometry,
                    restore,
                    crate::provision_support::BringUpContext {
                        outcome: crate::provision_failure::Abandonment::Keep,
                        lease: Some(lease),
                        handover: Some(handover),
                        prepared_skills: Some(prepared_skills),
                        rotation_claim,
                    },
                )
                .await;
        });
        Ok(moved.session)
    }

    /// Restore a coordinated rotation while its old authority still matches.
    pub(crate) async fn rollback_coordinated_handover(
        &self,
        original: &SessionRecord,
        rotating: &SessionRecord,
        lease: &mut SessionLease,
    ) {
        if let Some(restored) = self.restore_after_failed_move(original, rotating).await {
            let boundary =
                SessionBoundary::for_record(&restored, lease.boundary.authority.generation);
            if boundary != lease.boundary {
                if let Err(error) = self.advance_runtime_lease(lease, &boundary).await {
                    tracing::warn!(
                        session_id = %lease.session_id,
                        %error,
                        "the restored handover lease could not advance"
                    );
                }
            }
        }
        if let Err(error) = self.store.release_session_lease(lease).await {
            tracing::debug!(
                session_id = %lease.session_id,
                %error,
                "the failed handover lease ended before explicit release"
            );
        }
        self.finish_start(&lease.session_id).await;
    }

    /// Put a handover back on its original runtime after the final move failed.
    ///
    /// The old runtime and its entry have not been touched yet. Restoring the
    /// status makes their existing channels usable again; if this compensating
    /// write is also unavailable, leaving both sides `Rotating` is coherent and
    /// recoverable, while tearing anything down would not be.
    pub(crate) async fn restore_after_failed_move(
        &self,
        original: &SessionRecord,
        rotating: &SessionRecord,
    ) -> Option<SessionRecord> {
        let id = &original.session.id;
        // Roll back only the exact durable rotation claim made by this task.
        // Reloading by status and runtime would let a stale task erase a newer
        // replica's claim after an ABA transition back to `Rotating`.
        let mut restored = rotating.clone();
        restored.session.status = original.session.status;
        restored.session.stop_reason = original.session.stop_reason;
        restored.rotation_claim = None;
        restored.pending_handover = None;
        restored.skill_packs.clone_from(&original.skill_packs);
        let event = Event {
            session_id: id.clone(),
            seq: 0,
            kind: EventKind::StatusChanged {
                status: original.session.status,
                stop_reason: original.session.stop_reason,
                detail: Some(
                    "the handover could not be completed; continuing on the existing runtime"
                        .into(),
                ),
                capability: None,
            },
            appended_at: String::new(),
        };
        match self
            .roll_back_handover_durably(&mut restored, rotating, vec![event])
            .await
        {
            Ok(()) => {
                self.mark_entry(id, original.session.status, restored.session.last_seq)
                    .await;
                Some(restored)
            }
            Err(error) => {
                tracing::error!(
                    session_id = %id,
                    %error,
                    "cannot restore the handover status; leaving the old runtime intact"
                );
                None
            }
        }
    }

    /// Record that a session is mid-handover, and tell anyone watching.
    async fn mark_rotating(
        &self,
        record: &SessionRecord,
        change: &Handover,
        pending_handover: Option<PendingHandover>,
        source_generation: u64,
        prepared_skills: &crate::provision_support::PreparedSkillPacks,
    ) -> Option<SessionRecord> {
        let mut rotating = record.clone();
        rotating.session.status = SessionStatus::Rotating;
        rotating.session.stop_reason = None;
        rotating.rotation_claim = Some(pending_handover.as_ref().map_or_else(
            || format!("rotn_{}", uuid::Uuid::new_v4().simple()),
            |pending| pending.claim.clone(),
        ));
        rotating.pending_handover = pending_handover;
        if matches!(
            &record.skill_packs,
            crate::store::SkillPackState::Legacy
                | crate::store::SkillPackState::LegacyHandover { .. }
        ) {
            let Some(source_runtime_id) = record
                .runtime_id
                .as_ref()
                .filter(|runtime_id| !runtime_id.is_empty())
                .cloned()
            else {
                tracing::error!(
                    session_id = %record.session.id,
                    "cannot record legacy skill handover without an exact source runtime"
                );
                return None;
            };
            // The legacy candidate can differ from H1's old grant. Publish it
            // only with the noninteractive Rotating boundary; Ready later makes
            // the matching grant visible with the exact H2 runtime.
            rotating.skill_packs = crate::store::SkillPackState::LegacyHandover {
                source_runtime_id,
                source_generation,
                packs: prepared_skills.packs.clone(),
                stored_digest: prepared_skills.stored_digest,
            };
        }
        let id = rotating.session.id.clone();
        let events = vec![
            Event {
                session_id: id.clone(),
                seq: 0,
                kind: EventKind::RuntimeHandoverStarted,
                appended_at: String::new(),
            },
            Event {
                session_id: id.clone(),
                seq: 0,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Rotating,
                    stop_reason: None,
                    detail: Some(change.detail().to_string()),
                    capability: None,
                },
                appended_at: String::new(),
            },
        ];
        if let Err(e) = self
            .mark_handover_durably(&mut rotating, record, events)
            .await
        {
            tracing::error!(session_id = %id, error = %e, "cannot record the handover");
            return None;
        }
        // `persist_durably` is an exact write, so it does not advance the
        // semantic lineage used by later inheriting commits. Seed that lineage
        // with the record that owns this claim before conversation folds can
        // queue history-only snapshots whose entry does not carry the private
        // claim field. The final move can then clear this exact token while
        // still rebasing over all history written during the second snapshot.
        self.event_logs.lane(&id).observe_record(&rotating);
        let seq = rotating.session.last_seq;
        self.mark_entry(&id, SessionStatus::Rotating, seq).await;
        Some(rotating)
    }

    /// Keep a live entry's idea of its own status in step with the record.
    async fn mark_entry(&self, id: &SessionId, status: SessionStatus, seq: u64) {
        if let Some(entry) = self.entries.write().await.get_mut(id) {
            entry.session.status = status;
            entry.session.last_seq = seq;
        }
    }
}
