//! Coordination and record helpers for bringing a session runtime up.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{
    KernelAdapter, RuntimeCapacity, RuntimeHandle, RuntimeIdentity, RuntimeOptions,
    RuntimePermission,
};

use crate::capabilities::SessionCapabilities;
use crate::coordination::{
    SessionBoundary, SessionInteractionLeaseRequest, SessionLease, SessionLeaseRequest,
    SessionOperation,
};
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::options::{AgentOptions, PermissionMode};
use crate::service::{store_failed, SessionService};
use crate::session::{ProvisioningStage, Session, SessionSpec};
use crate::size::SessionSize;
use crate::terminal::TerminalGeometry;
use crate::StoreError;

/// How long a runtime may take to become usable.
///
/// Generous, because a cold image pull onto a fresh node is not fast and giving
/// up early would fail sessions that were about to work. Bounded all the same:
/// without a deadline a provider that never answers leaves a session waiting
/// for something that is not coming, and the sandbox it may already have
/// created is never reclaimed.
const PROVISION_DEADLINE: Duration = Duration::from_secs(600);

/// Long enough to cover one bounded provider create and attachment attempt.
/// Renewed at every durable/provider boundary rather than trusted for its age.
pub(crate) const RUNTIME_OPERATION_LEASE_TTL: Duration = Duration::from_secs(600);

/// Interaction work is short and frequent, so a crashed holder should clear quickly.
const INTERACTION_LEASE_TTL: Duration = Duration::from_secs(30);

/// Refresh well before expiry while a durable interaction commit is pending.
const INTERACTION_LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(10);

/// A transient store outage gets another chance while the current lease can still be valid.
const INTERACTION_LEASE_RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// Leave enough authority for the bounded sender to finish after its final check.
const INTERACTION_DELIVERY_MARGIN: Duration = Duration::from_secs(5);

/// Exact runtime boundary plus the live permission to begin one interaction step.
///
/// Interaction closures may wait on process-local gates after durable
/// acquisition. They must call [`Self::ensure_delivery_allowed`] immediately
/// before every external delivery, or immediately before a purely local
/// mutation. Once an external delivery returns a successful receipt, however,
/// its causally required local enqueue or durable commit is mandatory
/// service-owned finalization and must not be refused by checking this guard
/// again.
#[derive(Clone)]
pub(crate) struct RuntimeInteractionAuthority {
    boundary: SessionBoundary,
    validity: Arc<std::sync::Mutex<RuntimeInteractionValidity>>,
}

struct RuntimeInteractionValidity {
    deadline: Option<tokio::time::Instant>,
    invalid_reason: Option<String>,
}

/// Invalidates work that has not begun delivery when its caller stops waiting.
///
/// The spawned interaction still owns its action and lease cleanup. Invalidation
/// only makes its next delivery-boundary check fail, so an external send that
/// already returned a receipt can still finish its mandatory local work.
struct RuntimeInteractionCallerGuard {
    authority: Option<RuntimeInteractionAuthority>,
}

impl RuntimeInteractionCallerGuard {
    fn new(authority: RuntimeInteractionAuthority) -> Self {
        Self {
            authority: Some(authority),
        }
    }

    fn complete(&mut self) {
        self.authority = None;
    }
}

impl Drop for RuntimeInteractionCallerGuard {
    fn drop(&mut self) {
        if let Some(authority) = self.authority.take() {
            authority
                .invalidate("the interaction caller stopped before delivery could begin".into());
        }
    }
}

impl RuntimeInteractionAuthority {
    fn new(boundary: SessionBoundary, deadline: Option<tokio::time::Instant>) -> Self {
        Self {
            boundary,
            validity: Arc::new(std::sync::Mutex::new(RuntimeInteractionValidity {
                deadline,
                invalid_reason: None,
            })),
        }
    }

    /// Whether durable coordination already excludes concurrent lifecycle work.
    ///
    /// Compatibility interactions must retain the process-local
    /// lifecycle-before-input lock order. Durably fenced interactions can skip
    /// that lifecycle gate because the interaction lease excludes lifecycle
    /// operations at the store boundary.
    pub(crate) fn is_durably_fenced(&self) -> bool {
        self.deadline().is_some()
    }

    /// Whether a process-local sender belongs to this exact runtime generation.
    ///
    /// This comparison grants no delivery permission. Callers must still invoke
    /// [`Self::ensure_delivery_allowed`] after all local validation and
    /// immediately before the external send.
    pub(crate) fn fences_runtime_generation(&self, generation: u64) -> bool {
        self.boundary.authority.generation == generation
    }

    /// Refuse a new delivery without enough authority for its bounded send.
    ///
    /// This is the only boundary accessor so generation validation cannot be
    /// separated from the live validity check. Call it immediately before each
    /// external delivery, or before a purely local mutation. Do not call it
    /// again between a successful external receipt and the local enqueue or
    /// durable commit required to finalize that receipt.
    pub(crate) fn ensure_delivery_allowed(&self) -> Result<&SessionBoundary, CoreError> {
        let validity = self
            .validity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(reason) = validity.invalid_reason.as_ref() {
            return Err(CoreError::RuntimeUnavailable(reason.clone()));
        }
        if validity.deadline.is_some_and(|deadline| {
            deadline.saturating_duration_since(tokio::time::Instant::now())
                <= INTERACTION_DELIVERY_MARGIN
        }) {
            return Err(CoreError::RuntimeUnavailable(
                "the interaction lease cannot cover another bounded delivery".into(),
            ));
        }
        Ok(&self.boundary)
    }

    fn deadline(&self) -> Option<tokio::time::Instant> {
        self.validity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .deadline
    }

    fn extend(&self, deadline: tokio::time::Instant) {
        let mut validity = self
            .validity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if validity.invalid_reason.is_none() {
            validity.deadline = Some(deadline);
        }
    }

    fn invalidate(&self, reason: String) {
        let mut validity = self
            .validity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validity.invalid_reason.get_or_insert(reason);
    }
}

impl SessionService {
    /// Provision a runtime for an existing session and attach its terminal.
    pub(crate) async fn bring_up(
        self: &Arc<Self>,
        session: Session,
        spec: SessionSpec,
        adapter: Arc<dyn KernelAdapter>,
        geometry: TerminalGeometry,
        restore: Option<Vec<u8>>,
        outcome: crate::provision_failure::Abandonment,
    ) {
        self.bring_up_with_lease(
            session, adapter, spec, geometry, restore, outcome, None, None,
        )
        .await;
    }

    /// Bring a runtime up while continuing a lease acquired by a handover.
    /// Ordinary provisioning passes no lease and acquires one from its current
    /// durable boundary. Compatibility stores return `None` and retain the
    /// generation-zero path until their coordination protocol is promoted.
    pub(crate) async fn bring_up_with_lease(
        self: &Arc<Self>,
        session: Session,
        adapter: Arc<dyn KernelAdapter>,
        spec: SessionSpec,
        geometry: TerminalGeometry,
        restore: Option<Vec<u8>>,
        outcome: crate::provision_failure::Abandonment,
        mut lease: Option<SessionLease>,
        handover: Option<crate::rotate::CoordinatedHandover>,
    ) {
        let id = session.id.clone();
        /*
         * The session's own credential, minted against what it was granted.
         *
         * Unconditional, and that is the fix for a defect this gate used to
         * hide. It read `if the caller supplied one, use theirs` - and an
         * interface did supply one: the caller's own access token, put on the
         * spec so the platform could be asked questions as them. Provisioning
         * found the field filled, skipped the mint, and handed the agent its
         * owner's whole identity. Every session a person created ran that way,
         * while the minting machinery ran only for sessions the controller
         * recovered, which is the one path nobody takes by hand.
         *
         * `credential_for` is idempotent per session: asking twice - for a
         * replaced runtime, or after a restart - returns the value already
         * issued rather than leaving two alive for one session.
         */
        let mut spec = spec;
        let services = self.services_for(&session.id).await;
        let reaches: Vec<String> = services.iter().map(|service| service.0.clone()).collect();
        // Render here, where both inputs are authoritative: this service owns
        // the deployment setting and the session record owns its frozen grant.
        // Layer 3 receives prose and has no reason to understand either model.
        let platform_briefing = self
            .settings
            .for_provisioning()
            .await
            .briefing
            .render(&reaches);
        spec.runtime_credential = match self
            .credential_for(
                &session.id,
                &session.owner,
                &services,
                spec.caller_credential.as_ref(),
            )
            .await
        {
            Ok(credential) => credential,
            Err(error) => {
                tracing::error!(
                    session_id = %id,
                    error = %error,
                    "could not establish durable credential custody"
                );
                if let Err(cleanup_error) = self
                    .abandon(
                        &id,
                        &adapter,
                        &crate::provision_failure::StartFailure::transient(
                            "the durable credential store is unavailable".to_string(),
                        ),
                        outcome,
                    )
                    .await
                {
                    tracing::error!(
                        session_id = %id,
                        error = %cleanup_error,
                        "could not persist cleanup after credential custody failed"
                    );
                }
                return;
            }
        };
        if spec.runtime_credential.is_none() {
            tracing::warn!(
                session_id = %id,
                "no credential could be minted for this session; it will start unable to act \
                 on its owner's behalf until the top-up sweep supplies one"
            );
        }
        let operation = if session.status.is_rotating() {
            SessionOperation::Handover
        } else {
            SessionOperation::Provision
        };
        let mut coordination_error = false;
        let started = if lease.is_none() {
            match self.acquire_runtime_lease(&id, operation).await {
                Ok(acquired) => {
                    lease = acquired;
                    tokio::time::timeout(
                        PROVISION_DEADLINE,
                        self.start_with_retries(
                            &session,
                            &spec,
                            &adapter,
                            geometry,
                            restore.as_ref(),
                            &platform_briefing,
                            &mut lease,
                            handover.as_ref(),
                        ),
                    )
                    .await
                }
                Err(error) => {
                    coordination_error = true;
                    Ok(Err(crate::provision_failure::StartFailure::transient(
                        format!("durable runtime authority could not be acquired: {error}"),
                    )))
                }
            }
        } else {
            tokio::time::timeout(
                PROVISION_DEADLINE,
                self.start_with_retries(
                    &session,
                    &spec,
                    &adapter,
                    geometry,
                    restore.as_ref(),
                    &platform_briefing,
                    &mut lease,
                    handover.as_ref(),
                ),
            )
            .await
        };

        let failed = !matches!(&started, Ok(Ok(())));
        let can_restore_handover = if failed {
            match (&handover, lease.as_ref()) {
                (Some(handover), Some(_)) => {
                    let authority = self.store.runtime_authority(&id).await;
                    let current = self.store.get(&id).await;
                    matches!(authority, Ok(Some(ref authority)) if authority == &handover.rotating_boundary.authority)
                        && matches!(current, Ok(Some(ref record))
                            if record.session.status == handover.rotating.session.status
                                && record.runtime_id == handover.rotating.runtime_id
                                && record.rotation_claim == handover.rotating.rotation_claim)
                }
                _ => false,
            }
        } else {
            false
        };
        if can_restore_handover {
            let handover = handover
                .as_ref()
                .expect("a restorable handover has its exact durable context");
            self.rollback_coordinated_handover(
                &handover.original,
                &handover.rotating,
                lease
                    .as_mut()
                    .expect("a restorable handover still holds its lease"),
            )
            .await;
            return;
        }

        // A coordinated creation that never switched authority can still
        // record its ordinary terminal/resting outcome. Its failed candidates
        // remain cleanup debt, and the authoritative runtime is exactly none,
        // so this status write performs no provider mutation and erases no
        // cleanup identity.
        let can_abandon_without_authority = if failed && handover.is_none() {
            match lease.as_ref() {
                Some(held) if held.boundary.authority.runtime_id.is_none() => {
                    let authority = self.store.runtime_authority(&id).await;
                    let current = self.store.get(&id).await;
                    matches!(authority, Ok(Some(ref authority)) if authority == &held.boundary.authority)
                        && matches!(current, Ok(Some(ref record))
                            if record.session.status == held.boundary.status
                                && record.runtime_id.is_none())
                }
                _ => false,
            }
        } else {
            false
        };
        if can_abandon_without_authority {
            let abandonment = match &started {
                Ok(Err(failure)) => self.abandon(&id, &adapter, failure, outcome).await,
                Err(_) => {
                    let failure = crate::provision_failure::StartFailure::transient(format!(
                        "the runtime did not become usable within {}s",
                        PROVISION_DEADLINE.as_secs()
                    ));
                    self.abandon(&id, &adapter, &failure, outcome).await
                }
                Ok(Ok(())) => unreachable!("only a failed start is abandoned"),
            };
            if let Err(error) = abandonment {
                tracing::error!(
                    session_id = %id,
                    %error,
                    "cannot record the coordinated runtime abandonment; leaving its debt intact"
                );
            } else {
                let current = self.store.get(&id).await;
                if let (Some(held), Ok(Some(record))) = (lease.as_mut(), current) {
                    let boundary =
                        SessionBoundary::for_record(&record, held.boundary.authority.generation);
                    if let Err(error) = self.store.advance_session_lease(held, &boundary).await {
                        tracing::warn!(
                            session_id = %id,
                            %error,
                            "the abandonment committed but its runtime lease could not advance"
                        );
                    }
                }
            }
            if let Some(held) = lease.take() {
                if let Err(error) = self.store.release_session_lease(&held).await {
                    tracing::debug!(
                        session_id = %id,
                        %error,
                        "the abandoned runtime lease ended before explicit release"
                    );
                }
            }
            self.finish_start(&id).await;
            return;
        }

        let coordinated_failure = failed && (lease.is_some() || coordination_error);
        // A failure handler acquires its own lifecycle lease from the boundary
        // left by this attempt. Releasing first prevents an in-process failure
        // path from deadlocking behind its own still-active provider lease.
        if let Some(held) = lease.take() {
            if let Err(error) = self.store.release_session_lease(&held).await {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "runtime operation lease ended before explicit release"
                );
            }
        }

        match started {
            Ok(Ok(())) => {}
            Ok(Err(failure)) if coordinated_failure => {
                tracing::error!(
                    session_id = %id,
                    reason = %failure.reason,
                    "coordinated provisioning stopped; durable authority and cleanup debt remain for recovery"
                );
            }
            Ok(Err(failure)) => {
                if let Err(error) = self.abandon(&id, &adapter, &failure, outcome).await {
                    tracing::error!(
                        session_id = %id,
                        %error,
                        "cannot record the runtime abandonment; leaving it intact"
                    );
                }
            }
            Err(_) if coordinated_failure => {
                tracing::error!(
                    session_id = %id,
                    "coordinated provisioning timed out; durable authority and cleanup debt remain for recovery"
                );
            }
            Err(_) => {
                if let Err(error) = self
                    .abandon(
                        &id,
                        &adapter,
                        &crate::provision_failure::StartFailure::transient(format!(
                            "the runtime did not become usable within {}s",
                            PROVISION_DEADLINE.as_secs()
                        )),
                        outcome,
                    )
                    .await
                {
                    tracing::error!(
                        session_id = %id,
                        %error,
                        "cannot record the timed-out runtime abandonment; leaving it intact"
                    );
                }
            }
        }
        // Released last, and on every path: while the claim is held the
        // controller leaves this session alone, so failing to release it would
        // strand a session nothing is allowed to recover.
        self.finish_start(&id).await;
    }

    /// Acquire one exact durable provider-mutation boundary.
    pub(crate) async fn acquire_runtime_lease(
        &self,
        id: &SessionId,
        operation: SessionOperation,
    ) -> Result<Option<SessionLease>, StoreError> {
        let record = self
            .store
            .get(id)
            .await?
            .ok_or_else(|| StoreError::SessionDeleted { id: id.clone() })?;
        self.acquire_runtime_lease_at(&record, operation).await
    }

    /// Acquire against a caller's exact record snapshot.
    ///
    /// Rotation uses this form so a stale task cannot snapshot one runtime
    /// while obtaining authority over a newer record for the same session.
    pub(crate) async fn acquire_runtime_lease_at(
        &self,
        record: &crate::store::SessionRecord,
        operation: SessionOperation,
    ) -> Result<Option<SessionLease>, StoreError> {
        let id = &record.session.id;
        let authority = match self.store.runtime_authority(id).await {
            Ok(Some(authority)) => authority,
            Ok(None) => return Err(StoreError::SessionDeleted { id: id.clone() }),
            Err(StoreError::CoordinationDisabled) => return Ok(None),
            Err(error) => return Err(error),
        };
        let boundary = SessionBoundary::for_record(record, authority.generation);
        if operation == SessionOperation::Interact {
            let request = SessionInteractionLeaseRequest::new(
                id.clone(),
                self.coordination_holder.clone(),
                boundary,
                INTERACTION_LEASE_TTL,
            );
            self.store
                .acquire_interaction_lease(&request)
                .await
                .map(Some)
        } else {
            let request = SessionLeaseRequest::new(
                id.clone(),
                self.coordination_holder.clone(),
                boundary,
                operation,
                RUNTIME_OPERATION_LEASE_TTL,
            );
            self.store.acquire_session_lease(&request).await.map(Some)
        }
    }

    /// Run one interaction while its exact runtime authority remains fenced.
    ///
    /// The action must take any required process-local input gate itself.
    /// Acquiring the durable lease first is the lock order used by handover and
    /// prevents a local reader from holding that gate while waiting behind a
    /// handover that needs the corresponding writer. Durably fenced actions may
    /// skip a process-local lifecycle gate; compatibility actions must preserve
    /// the lifecycle-before-input lock order.
    ///
    /// Call [`RuntimeInteractionAuthority::ensure_delivery_allowed`]
    /// immediately before every external delivery, or before a purely local
    /// mutation. After an external delivery returns a successful receipt, its
    /// causally required local enqueue or durable commit is mandatory
    /// service-owned finalization: complete it without re-checking the deadline
    /// guard, even if the interaction lease expires while it settles.
    pub(crate) async fn run_runtime_interaction_at<T, A, F>(
        self: &Arc<Self>,
        record: &crate::store::SessionRecord,
        action: A,
    ) -> Result<T, CoreError>
    where
        T: Send + 'static,
        A: FnOnce(RuntimeInteractionAuthority) -> F + Send + 'static,
        F: Future<Output = Result<T, CoreError>> + Send + 'static,
    {
        // The request start is a conservative lower bound for the store's
        // authoritative acquisition time. If even this local window elapsed,
        // delivery must not begin against an already-expired exclusion.
        let acquisition_started = tokio::time::Instant::now();
        let lease = match self
            .acquire_runtime_lease_at(record, SessionOperation::Interact)
            .await
        {
            Ok(lease) => lease,
            Err(StoreError::SessionInteractionBlocked {
                operation: SessionOperation::Handover,
                ..
            }) => return Err(CoreError::SessionRotating),
            Err(StoreError::SessionAuthorityChanged { .. }) => {
                let current = self
                    .store
                    .get(&record.session.id)
                    .await
                    .map_err(store_failed)?;
                return match current {
                    None => Err(CoreError::SessionNotFound),
                    Some(current) if current.session.status.is_terminal() => {
                        Err(CoreError::SessionTerminated)
                    }
                    Some(current) if current.session.status.is_rotating() => {
                        Err(CoreError::SessionRotating)
                    }
                    Some(_) => Err(CoreError::RuntimeUnavailable(
                        "the session runtime changed before interaction delivery".into(),
                    )),
                };
            }
            Err(error) => return Err(store_failed(error)),
        };

        let boundary = lease.as_ref().map_or_else(
            || SessionBoundary::for_record(record, 0),
            |lease| lease.boundary.clone(),
        );
        let lease_deadline = lease
            .as_ref()
            .map(|_| acquisition_started + INTERACTION_LEASE_TTL);
        let authority = RuntimeInteractionAuthority::new(boundary, lease_deadline);
        let mut caller = RuntimeInteractionCallerGuard::new(authority.clone());
        let (finished, result) = tokio::sync::oneshot::channel();
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let action_result = service
                .run_owned_runtime_interaction(lease.as_ref(), authority, action)
                .await;
            // A release can commit and still lose or indefinitely delay its
            // response. Publish the known action result before starting it so
            // cleanup cannot invite a duplicate non-idempotent delivery.
            let _ = finished.send(action_result);

            let Some(lease) = lease else {
                return;
            };
            if let Err(first_error) = service.store.release_session_lease(&lease).await {
                if let Err(retry_error) = service.store.release_session_lease(&lease).await {
                    tracing::warn!(
                        session_id = %lease.session_id,
                        error = %retry_error,
                        first_error = %first_error,
                        "interaction lease release remained unresolved after exact retry"
                    );
                }
            }
        });
        let action_result = result.await.map_err(|_| {
            CoreError::RuntimeUnavailable("this session's interaction task stopped".into())
        });
        caller.complete();
        action_result?
    }

    /// Drive one action independently of its caller while maintaining its lease.
    async fn run_owned_runtime_interaction<T, A, F>(
        &self,
        lease: Option<&SessionLease>,
        authority: RuntimeInteractionAuthority,
        action: A,
    ) -> Result<T, CoreError>
    where
        A: FnOnce(RuntimeInteractionAuthority) -> F,
        F: Future<Output = Result<T, CoreError>>,
    {
        authority.ensure_delivery_allowed()?;
        let action = action(authority.clone());
        let Some(lease) = lease else {
            return action.await;
        };
        tokio::pin!(action);
        let mut next_renewal = authority
            .deadline()
            .expect("a durable interaction has a deadline")
            - (INTERACTION_LEASE_TTL - INTERACTION_LEASE_RENEW_INTERVAL);
        loop {
            tokio::select! {
                result = &mut action => return result,
                _ = tokio::time::sleep_until(next_renewal) => {
                    let renewal_started = tokio::time::Instant::now();
                    let deadline = authority
                        .deadline()
                        .expect("a durable interaction has a deadline");
                    if renewal_started >= deadline {
                        authority.invalidate(
                            "the interaction lease expired while delivery was settling".into()
                        );
                        tracing::warn!(
                            session_id = %lease.session_id,
                            "interaction lease expired while service-owned delivery was still settling"
                        );
                        return action.await;
                    }
                    match self
                        .store
                        .renew_interaction_lease(lease, INTERACTION_LEASE_TTL)
                        .await
                    {
                        Ok(()) => {
                            authority.extend(renewal_started + INTERACTION_LEASE_TTL);
                            next_renewal =
                                renewal_started + INTERACTION_LEASE_RENEW_INTERVAL;
                        }
                        Err(StoreError::Unavailable(error)) => {
                            tracing::debug!(
                                session_id = %lease.session_id,
                                %error,
                                "interaction lease renewal is temporarily unavailable"
                            );
                            next_renewal = (tokio::time::Instant::now()
                                + INTERACTION_LEASE_RETRY_INTERVAL)
                                .min(deadline);
                        }
                        Err(error) => {
                            authority.invalidate(format!(
                                "the interaction lease ended before delivery: {error}"
                            ));
                            tracing::warn!(
                                session_id = %lease.session_id,
                                %error,
                                "interaction lease ended while service-owned delivery was still settling"
                            );
                            return action.await;
                        }
                    }
                }
            }
        }
    }

    /// Refresh a coordinated runtime operation at a provider boundary.
    pub(crate) async fn renew_runtime_lease(
        &self,
        lease: Option<&SessionLease>,
    ) -> Result<(), StoreError> {
        match lease {
            Some(lease) => {
                self.store
                    .renew_session_lease(lease, RUNTIME_OPERATION_LEASE_TTL)
                    .await
            }
            None => Ok(()),
        }
    }

    /// Reclaim one debt while the operation that created it still owns the
    /// resulting no-runtime boundary.
    pub(crate) async fn reclaim_runtime_with_lease(
        &self,
        adapter: &Arc<dyn KernelAdapter>,
        lease: &SessionLease,
        cleanup: &crate::coordination::RuntimeCleanup,
    ) -> Result<(), String> {
        if cleanup.generation == 0 {
            let authorized = self
                .store
                .legacy_runtime_cleanup_authorized()
                .await
                .map_err(|error| error.to_string())?;
            if !authorized {
                return Err(
                    "legacy runtime cleanup is waiting for the old-writer-drain proof".into(),
                );
            }
        }
        self.renew_runtime_lease(Some(lease))
            .await
            .map_err(|error| error.to_string())?;
        let identity = RuntimeIdentity {
            handle: RuntimeHandle(cleanup.runtime_id.clone()),
            session: cleanup.session_id.0.clone(),
            generation: std::num::NonZeroU64::new(cleanup.generation),
        };
        adapter
            .terminate_if_identity(&identity)
            .await
            .map_err(|error| error.to_string())?;
        self.store
            .acknowledge_runtime_cleanup(lease, cleanup)
            .await
            .map_err(|error| error.to_string())
    }

    /// Retire the exact current authority with an idempotent store receipt.
    pub(crate) async fn retire_current_runtime(
        &self,
        lease: &mut SessionLease,
    ) -> Result<crate::coordination::RuntimeAuthorityRetirement, StoreError> {
        self.renew_runtime_lease(Some(lease)).await?;
        let token = format!("rret_{}", uuid::Uuid::new_v4().simple());
        match self.store.retire_runtime_authority(lease, &token).await {
            Ok(retired) => Ok(retired),
            Err(StoreError::Unavailable(_)) => {
                self.store.retire_runtime_authority(lease, &token).await
            }
            Err(error) => Err(error),
        }
    }

    /// Append a progress step to the session's log.
    ///
    /// Written durably rather than pushed at whoever happens to be watching,
    /// so someone who opens the page mid-start still sees where it got to. A
    /// step that cannot be recorded is logged and retained for the next
    /// session transaction: progress is commentary, and failing a session
    /// because its commentary failed would be absurd.
    pub(crate) async fn note_progress(&self, id: &SessionId, stage: ProvisioningStage) {
        let Ok(Some(mut record)) = self.store.get(id).await else {
            return;
        };
        if let Err(error) = self
            .persist_durably(
                &mut record,
                vec![Event {
                    session_id: id.clone(),
                    seq: 0,
                    kind: EventKind::Progress { stage },
                    appended_at: String::new(),
                }],
            )
            .await
        {
            tracing::warn!(session_id = %id, error = %error, "cannot record progress");
        }
    }

    /// Record the runtime a session was given, as soon as it exists.
    /// Write down the skills this runtime was actually given.
    ///
    /// Not the set the session asked for. A rotation has no caller to fetch as
    /// and falls back to this service's own identity, which sees only public
    /// skills - so a session can legitimately come back with fewer than it
    /// started with, and #442 is about stopping that. Until it does, the record
    /// must say what is on the disk rather than what was once requested.
    ///
    /// The difference is not cosmetic: the record is what every interface
    /// reports, so a record describing skills the sandbox does not have is an
    /// interface telling somebody their agent can do something it cannot.
    pub(crate) async fn remember_skills(&self, id: &SessionId, skills: &[crate::SkillPack]) {
        let installed: Vec<crate::environment::GrantedSkill> = skills
            .iter()
            .map(|pack| crate::environment::GrantedSkill {
                id: crate::environment::SkillId::new(pack.name.clone()),
                version: pack.version.clone(),
            })
            .collect();
        let Ok(Some(mut record)) = self.store.get(id).await else {
            tracing::error!(session_id = %id, "cannot record the skills: session is gone");
            return;
        };
        if record.session.granted_skills.as_deref() == Some(installed.as_slice()) {
            return;
        }
        tracing::info!(
            session_id = %id,
            had = ?record.session.granted_skills,
            now = ?installed,
            "recording the skills this runtime was actually given"
        );
        record.session.granted_skills = Some(installed);
        if let Err(e) = self.persist_durably(&mut record, Vec::new()).await {
            tracing::error!(
                session_id = %id,
                error = %e,
                "skills installed but not recorded; the record now overstates what this session has"
            );
        }
    }

    pub(crate) async fn remember_runtime(&self, id: &SessionId, handle: &RuntimeHandle) {
        let Ok(Some(mut record)) = self.store.get(id).await else {
            tracing::error!(session_id = %id, "cannot record the runtime: session is gone");
            return;
        };
        record.runtime_id = Some(handle.0.clone());
        // Stamped with what it was actually started on, so the next sweep can
        // tell a session that is current from one that is behind. Without this
        // every session would look due for rotation forever.
        record.session.runtime_version = self.runtime_version.clone();
        if let Err(e) = self.persist_durably(&mut record, Vec::new()).await {
            // Worth shouting about: the runtime exists and the record does not
            // name it, which is exactly the orphan reconciliation must catch.
            tracing::error!(session_id = %id, error = %e, "runtime provisioned but not recorded");
        }
    }

    /// Give a session abilities that were never recorded, and record them.
    ///
    /// Two jobs that must not be separated. It refuses a session whose runtime
    /// can no longer be what the session is - the check every start makes -
    /// and it writes the answer down where the session was recorded before
    /// this model existed.
    ///
    /// The second job is why this exists. `granted` is the only thing that
    /// produces a set, and until now the two paths that bring a session *back*
    /// — readopting a surviving runtime, and starting a replacement — either
    /// copied the record verbatim or computed the value and discarded it. A
    /// session written by an older build therefore came back holding nothing,
    /// which is not "granted nothing": it is refused at every gate that asks,
    /// including the one behind every keystroke.
    ///
    /// This also settles a single field added after the set existed. Filling a
    /// field that was never written does not change a recorded grant: there is
    /// no earlier answer to overwrite. Every field that was recorded remains
    /// untouched.
    ///
    /// Recorded rather than recomputed per process, because the record is the
    /// authority and a set that exists only in memory is one the next control
    /// plane has to derive again.
    pub(crate) async fn settle_capabilities(
        &self,
        session: &Session,
        adapter: &Arc<dyn KernelAdapter>,
    ) -> Result<SessionCapabilities, crate::error::CoreError> {
        let granted = SessionCapabilities::granted(session.options.mode, adapter.capabilities())?;
        if !session.capabilities.unrecorded() && session.capabilities.resources.is_some() {
            return Ok(session.capabilities);
        }
        let fallback = if session.capabilities.unrecorded() {
            granted
        } else {
            SessionCapabilities {
                resources: granted.resources,
                ..session.capabilities
            }
        };
        match self.store.get(&session.id).await {
            Ok(Some(mut record)) => {
                let current = record.session.capabilities;
                let (settled, resources_only) = if current.unrecorded() {
                    (granted, false)
                } else if current.resources.is_none() {
                    (
                        SessionCapabilities {
                            resources: granted.resources,
                            ..current
                        },
                        true,
                    )
                } else {
                    return Ok(current);
                };
                record.session.capabilities = settled;
                if let Err(e) = self.persist_durably(&mut record, Vec::new()).await {
                    // Worth saying and not worth refusing over: the session
                    // works with the set in hand, and the next sweep settles
                    // it again.
                    tracing::warn!(session_id = %session.id, error = %e, "cannot record what this session holds");
                } else if resources_only {
                    tracing::info!(
                        session_id = %session.id,
                        "settled resource reporting; this ability was never recorded"
                    );
                } else {
                    tracing::info!(
                        session_id = %session.id,
                        "settled what this session holds; it was recorded before capabilities existed"
                    );
                }
                Ok(settled)
            }
            Ok(None) => Ok(fallback),
            Err(e) => {
                tracing::warn!(session_id = %session.id, error = %e, "cannot read this session to settle what it holds");
                Ok(fallback)
            }
        }
    }
}

/// Translate what a person asked for into what a runtime is told.
///
/// The two vocabularies are separate on purpose - the core must not speak any
/// runtime's dialect - so this is the single place they meet, and the only
/// place a new option has to be taught to cross.
pub(crate) fn runtime_options(options: &AgentOptions) -> RuntimeOptions {
    RuntimeOptions {
        mode: match options.mode {
            crate::options::SessionMode::Terminal => layer3_adapter::RuntimeMode::Terminal,
            crate::options::SessionMode::Conversation => layer3_adapter::RuntimeMode::Conversation,
        },
        model: options.model.clone(),
        reasoning_effort: Some(options.reasoning_effort.as_str().to_string()),
        permission: match options.permission {
            PermissionMode::Guarded => RuntimePermission::Guarded,
            PermissionMode::Autonomous => RuntimePermission::Autonomous,
            PermissionMode::Unrestricted => RuntimePermission::Unrestricted,
        },
        own_credential: matches!(options.credential, crate::options::CredentialMode::Own),
        // Nothing to rejoin: this is a runtime being started, which has no
        // conversation yet. Taking one back over is the readopt path's, and it
        // reads the name from the record rather than from anything held here.
        resume_conversation: None,
    }
}

/// Translate the size a person asked for into what a runtime is given.
///
/// Beside `runtime_options` and for the same reason: the two vocabularies are
/// separate on purpose, so this is the one place they meet. `None` crosses as
/// `None` - the caller states nothing and the adapter shapes the runtime the
/// way this deployment shapes every runtime.
pub(crate) fn runtime_capacity(size: Option<SessionSize>) -> Option<RuntimeCapacity> {
    size.map(|size| RuntimeCapacity {
        cpu_millis: size.cpu_millis,
        memory_mib: size.memory_mib,
    })
}
