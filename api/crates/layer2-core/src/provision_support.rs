//! Coordination and record helpers for bringing a session runtime up.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
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
use crate::log::{CleanupAttempt, DeferredCleanup};
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
pub(crate) const PROVISION_DEADLINE: Duration = Duration::from_secs(600);

/// Long enough to cover one bounded provider create and attachment attempt.
/// Renewed at every durable/provider boundary rather than trusted for its age.
pub(crate) const RUNTIME_OPERATION_LEASE_TTL: Duration = Duration::from_secs(600);
/// Object I/O must finish while a freshly renewed runtime-operation lease is live.
const SKILL_PACK_IO_DEADLINE: Duration = Duration::from_secs(180);
/// Refresh a pending conversation before either it or its backing operation can expire.
pub(crate) const CONVERSATION_OPENING_RENEW_INTERVAL: Duration = Duration::from_secs(300);

/// Authority and failure disposition carried through one detached bring-up.
pub(crate) struct BringUpContext {
    pub(crate) outcome: crate::provision_failure::Abandonment,
    pub(crate) lease: Option<SessionLease>,
    pub(crate) handover: Option<crate::rotate::CoordinatedHandover>,
    /// Exact bytes resolved before H1 was touched, when this is a handover.
    pub(crate) prepared_skills: Option<PreparedSkillPacks>,
    /// Exact durable rotation claim this bring-up must publish or abandon.
    pub(crate) rotation_claim: Option<String>,
}

impl BringUpContext {
    pub(crate) fn compatibility(
        outcome: crate::provision_failure::Abandonment,
        rotation_claim: Option<String>,
    ) -> Self {
        Self {
            outcome,
            lease: None,
            handover: None,
            prepared_skills: None,
            rotation_claim,
        }
    }

    pub(crate) fn with_prepared_skills(mut self, prepared: PreparedSkillPacks) -> Self {
        self.prepared_skills = Some(prepared);
        self
    }
}

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
    /// Conservative lower bound used to authorize a new bounded delivery.
    deadline: Option<tokio::time::Instant>,
    /// Upper bound after which the store-issued lease is certain to be expired.
    lease_expiry_bound: Option<tokio::time::Instant>,
    /// A renewal may still commit after its local waiter loses the outcome.
    lease_expiry_unknown: bool,
    invalid_reason: Option<String>,
}

/// Why a completed interaction result could not be observed by its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeInteractionDeliveryLoss {
    /// The caller was gone before the service-owned task could publish its result.
    SendFailed,
    /// Publication succeeded, but the caller dropped the result before polling it.
    Unread,
}

/// Exact interaction authority that can move into restartable continuation work.
///
/// Every clone is an ownership responsibility. Dropping the final clone enters
/// exact lease release into service-owned cleanup synchronously, including when
/// no Tokio runtime exists on the dropping thread.
pub(crate) struct RetainedRuntimeInteraction {
    state: Arc<RetainedRuntimeInteractionState>,
}

struct RetainedRuntimeInteractionState {
    service: Weak<SessionService>,
    session_id: SessionId,
    lease: Option<SessionLease>,
    authority: RuntimeInteractionAuthority,
    owners: AtomicUsize,
    release_retained: AtomicBool,
}

struct DeferredRuntimeInteractionRelease {
    state: Arc<RetainedRuntimeInteractionState>,
}

enum UnknownInteractionAcquisitionPhase {
    Acquire,
    Release(SessionLease),
}

struct DeferredUnknownInteractionAcquisition {
    store: Arc<dyn crate::store::SessionStore>,
    request: SessionInteractionLeaseRequest,
    phase: UnknownInteractionAcquisitionPhase,
}

struct RuntimeInteractionAcquisitionGuard {
    event_logs: Arc<crate::log::EventLogs>,
    store: Arc<dyn crate::store::SessionStore>,
    request: Option<SessionInteractionLeaseRequest>,
    attempted: bool,
}

impl RetainedRuntimeInteraction {
    fn new(
        service: &Arc<SessionService>,
        session_id: SessionId,
        lease: Option<SessionLease>,
        authority: RuntimeInteractionAuthority,
    ) -> Self {
        Self {
            state: Arc::new(RetainedRuntimeInteractionState {
                service: Arc::downgrade(service),
                session_id,
                lease,
                authority,
                owners: AtomicUsize::new(1),
                release_retained: AtomicBool::new(false),
            }),
        }
    }

    /// A clone of the exact runtime boundary and shared delivery validity.
    pub(crate) fn authority(&self) -> RuntimeInteractionAuthority {
        self.state.authority.clone()
    }

    /// Renew the exact retained lease before a restarted continuation delivers.
    pub(crate) async fn resume_before_delivery(
        &self,
    ) -> Result<RuntimeInteractionAuthority, CoreError> {
        let authority = self.authority();
        authority.ensure_not_invalidated()?;
        let Some(lease) = self.state.lease.as_ref() else {
            authority.ensure_delivery_allowed()?;
            return Ok(authority);
        };
        let service = self.state.service.upgrade().ok_or_else(|| {
            CoreError::RuntimeUnavailable(
                "the interaction service stopped before delivery could resume".into(),
            )
        })?;

        loop {
            authority.ensure_not_invalidated()?;
            let renewal_started = tokio::time::Instant::now();
            let deadline = authority
                .deadline()
                .expect("a retained durable interaction has a deadline");
            if renewal_started >= deadline {
                authority.invalidate(
                    "the interaction lease expired before retained delivery resumed".into(),
                );
                authority.ensure_delivery_allowed()?;
                return Ok(authority);
            }
            let mut renewal = RuntimeInteractionRenewalGuard::new(authority.clone());
            let outcome = service
                .store
                .renew_interaction_lease(lease, INTERACTION_LEASE_TTL)
                .await;
            match outcome {
                Ok(()) => {
                    renewal.known();
                    authority.extend(
                        renewal_started + INTERACTION_LEASE_TTL,
                        tokio::time::Instant::now() + INTERACTION_LEASE_TTL,
                    );
                    authority.ensure_delivery_allowed()?;
                    return Ok(authority);
                }
                Err(StoreError::Unavailable(error)) => {
                    renewal.unknown();
                    tracing::debug!(
                        session_id = %lease.session_id,
                        %error,
                        "retained interaction lease renewal is temporarily unavailable"
                    );
                    let retry_at = (tokio::time::Instant::now() + INTERACTION_LEASE_RETRY_INTERVAL)
                        .min(deadline);
                    tokio::time::sleep_until(retry_at).await;
                }
                Err(error) => {
                    renewal.known();
                    authority.invalidate(format!(
                        "the interaction lease ended before retained delivery: {error}"
                    ));
                    authority.ensure_delivery_allowed()?;
                    return Ok(authority);
                }
            }
        }
    }
}

impl Clone for RetainedRuntimeInteraction {
    fn clone(&self) -> Self {
        self.state
            .owners
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |owners| {
                owners.checked_add(1)
            })
            .expect("retained interaction ownership count overflowed");
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl Drop for RetainedRuntimeInteraction {
    fn drop(&mut self) {
        let owners = self.state.owners.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(owners > 0, "retained interaction ownership underflowed");
        if owners == 1 {
            self.state.retain_release();
        }
    }
}

impl RetainedRuntimeInteractionState {
    fn retain_release(self: &Arc<Self>) {
        self.authority
            .invalidate("the retained interaction ended before delivery could begin".into());
        if self.lease.is_none() || self.release_retained.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(service) = self.service.upgrade() else {
            return;
        };
        service
            .event_logs
            .retain_cleanup(DeferredRuntimeInteractionRelease {
                state: Arc::clone(self),
            });
    }
}

impl DeferredCleanup for DeferredRuntimeInteractionRelease {
    fn session_id(&self) -> &SessionId {
        &self.state.session_id
    }

    fn attempt(&mut self) -> CleanupAttempt<'_> {
        Box::pin(async move {
            let Some(service) = self.state.service.upgrade() else {
                return;
            };
            let Some(lease) = self.state.lease.as_ref() else {
                return;
            };

            loop {
                let expiry = self.state.authority.lease_expiry_bound();
                if expiry.is_some_and(|expiry| tokio::time::Instant::now() >= expiry) {
                    return;
                }

                let outcome = match expiry {
                    Some(expiry) => {
                        let release = service.store.release_session_lease(lease);
                        tokio::pin!(release);
                        tokio::select! {
                            outcome = &mut release => outcome,
                            _ = tokio::time::sleep_until(expiry) => return,
                        }
                    }
                    None => service.store.release_session_lease(lease).await,
                };
                match outcome {
                    Ok(()) => return,
                    Err(StoreError::Unavailable(error)) => {
                        tracing::debug!(
                            session_id = %lease.session_id,
                            %error,
                            "interaction lease release is temporarily unavailable"
                        );
                        let retry_at = expiry.map_or_else(
                            || tokio::time::Instant::now() + INTERACTION_LEASE_RETRY_INTERVAL,
                            |expiry| {
                                (tokio::time::Instant::now() + INTERACTION_LEASE_RETRY_INTERVAL)
                                    .min(expiry)
                            },
                        );
                        tokio::time::sleep_until(retry_at).await;
                    }
                    Err(error) => {
                        tracing::warn!(
                            session_id = %lease.session_id,
                            %error,
                            "exact interaction lease release was rejected"
                        );
                        return;
                    }
                }
            }
        })
    }
}

impl DeferredCleanup for DeferredUnknownInteractionAcquisition {
    fn session_id(&self) -> &SessionId {
        &self.request.session_id
    }

    fn attempt(&mut self) -> CleanupAttempt<'_> {
        Box::pin(async move {
            loop {
                match &self.phase {
                    UnknownInteractionAcquisitionPhase::Acquire => {
                        match self.store.acquire_interaction_lease(&self.request).await {
                            Ok(lease) => {
                                // Record exact ownership before another await so
                                // interruption can only retry its exact release.
                                self.phase = UnknownInteractionAcquisitionPhase::Release(lease);
                            }
                            Err(StoreError::Unavailable(error)) => {
                                tracing::debug!(
                                    session_id = %self.request.session_id,
                                    %error,
                                    "unknown interaction acquisition recovery is temporarily unavailable"
                                );
                                tokio::time::sleep(INTERACTION_LEASE_RETRY_INTERVAL).await;
                            }
                            Err(error) => {
                                tracing::debug!(
                                    session_id = %self.request.session_id,
                                    %error,
                                    "unknown interaction acquisition no longer owns an exact lease"
                                );
                                return;
                            }
                        }
                    }
                    UnknownInteractionAcquisitionPhase::Release(lease) => {
                        match self.store.release_session_lease(lease).await {
                            Ok(()) => return,
                            Err(StoreError::Unavailable(error)) => {
                                tracing::debug!(
                                    session_id = %self.request.session_id,
                                    %error,
                                    "unknown interaction acquisition release is temporarily unavailable"
                                );
                                tokio::time::sleep(INTERACTION_LEASE_RETRY_INTERVAL).await;
                            }
                            Err(error) => {
                                tracing::warn!(
                                    session_id = %self.request.session_id,
                                    %error,
                                    "unknown interaction acquisition exact release was rejected"
                                );
                                return;
                            }
                        }
                    }
                }
            }
        })
    }
}

impl RuntimeInteractionAcquisitionGuard {
    fn new(service: &Arc<SessionService>, request: SessionInteractionLeaseRequest) -> Self {
        Self {
            event_logs: Arc::clone(&service.event_logs),
            store: Arc::clone(&service.store),
            request: Some(request),
            attempted: false,
        }
    }

    fn attempted(&mut self) {
        self.attempted = true;
    }

    fn definitive(&mut self) {
        self.attempted = false;
    }

    fn disarm(&mut self) {
        self.request.take();
        self.attempted = false;
    }
}

impl Drop for RuntimeInteractionAcquisitionGuard {
    fn drop(&mut self) {
        if !self.attempted {
            return;
        }
        let Some(request) = self.request.take() else {
            return;
        };
        self.event_logs
            .retain_cleanup(DeferredUnknownInteractionAcquisition {
                store: Arc::clone(&self.store),
                request,
                phase: UnknownInteractionAcquisitionPhase::Acquire,
            });
    }
}

type RuntimeInteractionUndelivered<T> = Box<
    dyn FnOnce(RuntimeInteractionDeliveryLoss, Result<T, CoreError>, RetainedRuntimeInteraction)
        + Send,
>;

/// A published interaction result that retains cleanup until caller observation.
struct RetainedRuntimeInteractionDelivery<T> {
    output: Option<Result<T, CoreError>>,
    interaction: Option<RetainedRuntimeInteraction>,
    on_undelivered: Option<RuntimeInteractionUndelivered<T>>,
}

impl<T> RetainedRuntimeInteractionDelivery<T> {
    fn new<U>(
        output: Result<T, CoreError>,
        interaction: RetainedRuntimeInteraction,
        on_undelivered: U,
    ) -> Self
    where
        U: FnOnce(RuntimeInteractionDeliveryLoss, Result<T, CoreError>, RetainedRuntimeInteraction)
            + Send
            + 'static,
    {
        Self {
            output: Some(output),
            interaction: Some(interaction),
            on_undelivered: Some(Box::new(on_undelivered)),
        }
    }

    fn observed(mut self) -> Result<T, CoreError> {
        let output = self
            .output
            .take()
            .expect("an interaction result is observed at most once");
        drop(self.interaction.take());
        drop(self.on_undelivered.take());
        output
    }

    fn report_undelivered(&mut self, reason: RuntimeInteractionDeliveryLoss) {
        let (Some(output), Some(interaction), Some(on_undelivered)) = (
            self.output.take(),
            self.interaction.take(),
            self.on_undelivered.take(),
        ) else {
            return;
        };
        interaction
            .state
            .authority
            .invalidate("the interaction caller stopped before delivery could begin".into());
        on_undelivered(reason, output, interaction);
    }
}

impl<T> Drop for RetainedRuntimeInteractionDelivery<T> {
    fn drop(&mut self) {
        self.report_undelivered(RuntimeInteractionDeliveryLoss::Unread);
    }
}

/// Invalidates work that has not begun delivery when its caller stops waiting.
///
/// The spawned interaction still owns its action and lease cleanup. Invalidation
/// only makes its next delivery-boundary check fail, so an external send that
/// already returned a receipt can still finish its mandatory local work.
struct RuntimeInteractionCallerGuard {
    state: Arc<RuntimeInteractionCallerState>,
    completed: bool,
}

struct RuntimeInteractionCallerState {
    active: AtomicBool,
    authority: std::sync::Mutex<Option<RuntimeInteractionAuthority>>,
    stopped: tokio::sync::Notify,
}

/// Tracks whether an in-flight renewal's outcome is known.
///
/// A lost response or a canceled waiter does not prove the renewal failed. In
/// that case exact cleanup cannot rely on any finite expiry bound, because the
/// store may still commit the renewal after the local future is dropped.
struct RuntimeInteractionRenewalGuard {
    authority: RuntimeInteractionAuthority,
    outcome_known: bool,
}

impl RuntimeInteractionCallerGuard {
    fn new() -> Self {
        Self {
            state: Arc::new(RuntimeInteractionCallerState {
                active: AtomicBool::new(true),
                authority: std::sync::Mutex::new(None),
                stopped: tokio::sync::Notify::new(),
            }),
            completed: false,
        }
    }

    fn state(&self) -> Arc<RuntimeInteractionCallerState> {
        Arc::clone(&self.state)
    }

    fn complete(&mut self) {
        self.completed = true;
        let mut authority = self
            .state
            .authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.state.active.store(false, Ordering::Release);
        authority.take();
        drop(authority);
        self.state.stopped.notify_waiters();
    }
}

impl Drop for RuntimeInteractionCallerGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let mut registered = self
            .state
            .authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.state.active.store(false, Ordering::Release);
        if let Some(authority) = registered.take() {
            authority
                .invalidate("the interaction caller stopped before delivery could begin".into());
        }
        drop(registered);
        self.state.stopped.notify_waiters();
    }
}

impl RuntimeInteractionCallerState {
    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn register_authority(&self, authority: RuntimeInteractionAuthority) -> bool {
        let mut registered = self
            .authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.is_active() {
            authority
                .invalidate("the interaction caller stopped before delivery could begin".into());
            return false;
        }
        *registered = Some(authority);
        true
    }

    async fn stopped(&self) {
        loop {
            let stopped = self.stopped.notified();
            tokio::pin!(stopped);
            stopped.as_mut().enable();
            if !self.is_active() {
                return;
            }
            stopped.await;
        }
    }
}

impl RuntimeInteractionRenewalGuard {
    fn new(authority: RuntimeInteractionAuthority) -> Self {
        Self {
            authority,
            outcome_known: false,
        }
    }

    fn known(&mut self) {
        self.outcome_known = true;
    }

    fn unknown(&mut self) {
        self.authority.mark_lease_expiry_unknown();
        self.outcome_known = true;
    }
}

impl Drop for RuntimeInteractionRenewalGuard {
    fn drop(&mut self) {
        if !self.outcome_known {
            self.authority.mark_lease_expiry_unknown();
        }
    }
}

impl RuntimeInteractionAuthority {
    fn new(
        boundary: SessionBoundary,
        deadline: Option<tokio::time::Instant>,
        lease_expiry_bound: Option<tokio::time::Instant>,
    ) -> Self {
        Self {
            boundary,
            validity: Arc::new(std::sync::Mutex::new(RuntimeInteractionValidity {
                deadline,
                lease_expiry_bound,
                lease_expiry_unknown: false,
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

    /// Refuse delivery unless the process-local channel belongs to the exact
    /// durable runtime authority acquired for this interaction.
    ///
    /// Retirement deliberately clears the runtime handle without advancing its
    /// generation. Comparing only the generation would therefore let another
    /// replica keep delivering through a retired channel after a normal stop.
    pub(crate) fn ensure_runtime_delivery_allowed(
        &self,
        runtime_id: Option<&str>,
        generation: u64,
    ) -> Result<&SessionBoundary, CoreError> {
        let boundary = self.ensure_delivery_allowed()?;
        if runtime_id.is_none()
            || boundary.authority.generation != generation
            || boundary.authority.runtime_id.as_deref() != runtime_id
        {
            return Err(CoreError::RuntimeUnavailable(
                "the live channel belongs to another runtime authority".into(),
            ));
        }
        Ok(boundary)
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

    fn lease_expiry_bound(&self) -> Option<tokio::time::Instant> {
        let validity = self
            .validity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (!validity.lease_expiry_unknown)
            .then_some(validity.lease_expiry_bound)
            .flatten()
    }

    fn mark_lease_expiry_unknown(&self) {
        let mut validity = self
            .validity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validity.lease_expiry_unknown = true;
    }

    fn extend(&self, deadline: tokio::time::Instant, lease_expiry_bound: tokio::time::Instant) {
        let mut validity = self
            .validity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Delivery permission and lease lifetime are independent. Caller
        // cancellation forbids the next send without ending the owned action,
        // which still needs renewed exclusion while it reaches that check or
        // settles mandatory post-receipt finalization.
        validity.deadline = Some(
            validity
                .deadline
                .map_or(deadline, |held| held.max(deadline)),
        );
        validity.lease_expiry_bound = Some(
            validity
                .lease_expiry_bound
                .map_or(lease_expiry_bound, |held| held.max(lease_expiry_bound)),
        );
    }

    fn ensure_not_invalidated(&self) -> Result<(), CoreError> {
        let validity = self
            .validity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match validity.invalid_reason.as_ref() {
            Some(reason) => Err(CoreError::RuntimeUnavailable(reason.clone())),
            None => Ok(()),
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

enum RuntimeInteractionAcquisition {
    Acquired {
        lease: Option<SessionLease>,
        started: tokio::time::Instant,
        completed: tokio::time::Instant,
    },
    CallerGone,
    Unknown {
        report: Option<CoreError>,
    },
}

/// Exact skill bytes retained across provider retries in one bring-up.
#[derive(Clone)]
pub(crate) struct PreparedSkillPacks {
    pub(crate) packs: Vec<crate::store::SkillPack>,
    pub(crate) stored_digest: Option<crate::store::SkillPackDigest>,
}

impl PreparedSkillPacks {
    fn new(packs: Vec<crate::store::SkillPack>, externally_stored: bool) -> Self {
        let stored_digest =
            externally_stored.then(|| crate::store::SkillPackDigest::for_packs(&packs));
        Self {
            packs,
            stored_digest,
        }
    }
}

impl SessionService {
    /// Resolve a new session's skills while its caller is still present.
    ///
    /// The returned bytes and the grant derived from them enter the atomic
    /// session create together. Nothing after that boundary needs the caller's
    /// credential, so a process loss cannot turn a private grant into an empty
    /// one merely because reconciliation has no request to borrow authority
    /// from.
    pub(crate) async fn resolve_skill_packs_for_creation(
        &self,
        caller_credential: Option<&str>,
        wanted: &[String],
    ) -> Result<Vec<crate::store::SkillPack>, crate::error::CoreError> {
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let source = self.skills.as_ref().ok_or_else(|| {
            crate::error::CoreError::SkillCatalogUnavailable(
                "this deployment has no source for the selected skills".into(),
            )
        })?;
        let wanted = wanted
            .iter()
            .cloned()
            .map(crate::environment::SkillId::new)
            .collect::<Vec<_>>();
        source
            .packs_for(caller_credential, &wanted)
            .await
            .map_err(crate::error::CoreError::SkillCatalogUnavailable)
    }

    pub(crate) fn granted_skills_for(
        packs: &[crate::store::SkillPack],
    ) -> Vec<crate::environment::GrantedSkill> {
        packs
            .iter()
            .map(|pack| crate::environment::GrantedSkill {
                id: crate::environment::SkillId::new(pack.name.clone()),
                version: pack.version.clone(),
            })
            .collect()
    }

    /// Resolve the exact bytes H2 will receive before H1 can be touched.
    ///
    /// Legacy registry fallback remains nonfatal, but its result stays in this
    /// handover's cache until the Rotating commit durably stages it. Modern
    /// sessions are read only from their recorded custody and never refetched.
    pub(crate) async fn prepare_skill_packs_for_handover(
        &self,
        session: &Session,
        lease: Option<&SessionLease>,
    ) -> Result<PreparedSkillPacks, crate::provision_failure::StartFailure> {
        let environment = self
            .environments
            .resolve_for_owner(&session.owner, session.environment_profile.as_ref())
            .await
            .map_err(|error| {
                crate::provision_failure::StartFailure::transient(format!(
                    "this session's environment could not be prepared before handover: {error}"
                ))
            })?;
        let mut prepared = None;
        self.prepare_skill_packs(session, &environment.skills, &mut prepared, false, lease)
            .await?
            .ok_or_else(|| {
                crate::provision_failure::StartFailure::transient(
                    "this session changed before its skills were prepared".into(),
                )
            })
    }

    /// Restore exact durable bytes, or perform the one nonfatal fallback that
    /// legacy sessions require. Provider retries retain the returned value and
    /// never ask the mutable registry twice.
    pub(crate) async fn prepare_skill_packs(
        &self,
        session: &Session,
        legacy_wanted: &[String],
        prepared: &mut Option<PreparedSkillPacks>,
        stage_legacy: bool,
        lease: Option<&SessionLease>,
    ) -> Result<Option<PreparedSkillPacks>, crate::provision_failure::StartFailure> {
        use crate::store::SkillPackState;

        let id = &session.id;
        let _lifecycle = self.lock_lifecycle(id).await;
        let mut record = match self.store.get(id).await {
            Ok(Some(record))
                if record.session.wants_runtime() && record.session.kernel == session.kernel =>
            {
                record
            }
            Ok(_) => return Ok(None),
            Err(error) => {
                return Err(crate::provision_failure::StartFailure::transient(format!(
                    "this session could not be checked before its skills were prepared: {error}"
                )))
            }
        };

        if matches!(&record.skill_packs, SkillPackState::StagedSummary) {
            return Err(crate::provision_failure::StartFailure::transient(
                "the session store returned a projected skill custody summary for an exact read"
                    .into(),
            ));
        }
        if matches!(
            &record.skill_packs,
            SkillPackState::CleanupPending | SkillPackState::Cleaned
        ) {
            return Ok(None);
        }
        if let Some(cached) = prepared.as_ref() {
            let custody_matches = match &record.skill_packs {
                SkillPackState::Staged(packs) => packs == &cached.packs,
                SkillPackState::Stored { digest } => cached.stored_digest == Some(*digest),
                SkillPackState::LegacyHandover {
                    packs,
                    stored_digest,
                    ..
                } => packs == &cached.packs && stored_digest == &cached.stored_digest,
                SkillPackState::Legacy
                | SkillPackState::StagedSummary
                | SkillPackState::CleanupPending
                | SkillPackState::Cleaned => false,
            };
            if !custody_matches {
                return Err(crate::provision_failure::StartFailure::transient(
                    "this session's skill custody changed during provider retry".into(),
                ));
            }
            return Ok(Some(cached.clone()));
        }

        let (packs, externally_stored) = match record.skill_packs.clone() {
            SkillPackState::Staged(packs) => {
                if record.session.status.is_interactive() {
                    self.require_matching_skill_grant(&record, &packs)?;
                }
                self.validate_prepared_skill_packs(&packs)?;
                let stored = self.store_staged_skill_packs(id, &packs, lease).await?;
                (packs, stored)
            }
            SkillPackState::Stored { digest } => {
                let packs = self.read_stored_skill_packs(id, lease).await?;
                if crate::store::SkillPackDigest::for_packs(&packs) != digest {
                    return Err(crate::provision_failure::StartFailure::transient(
                        "this session's stored skill packs failed their integrity check".into(),
                    ));
                }
                self.validate_prepared_skill_packs(&packs)?;
                self.require_matching_skill_grant(&record, &packs)?;
                tracing::info!(
                    session_id = %id,
                    skills = packs.len(),
                    "restored this session's stored skills"
                );
                (packs, true)
            }
            SkillPackState::LegacyHandover {
                source_runtime_id,
                packs,
                stored_digest,
                ..
            } => {
                if source_runtime_id.is_empty() {
                    return Err(crate::provision_failure::StartFailure::transient(
                        "this session's legacy skill handover has no exact source runtime".into(),
                    ));
                }
                self.validate_prepared_skill_packs(&packs)?;
                if stored_digest.is_some_and(|digest| {
                    crate::store::SkillPackDigest::for_packs(&packs) != digest
                }) {
                    return Err(crate::provision_failure::StartFailure::transient(
                        "this session's legacy handover skill receipt does not match its exact bytes"
                            .into(),
                    ));
                }
                // Replacing the deterministic object with the same candidate is
                // safe and repairs a lost object without consulting the mutable
                // registry. Without an external store the inline bytes remain
                // the durable retry source.
                let stored = self.store_staged_skill_packs(id, &packs, lease).await?;
                (packs, stored)
            }
            SkillPackState::Legacy => {
                let packs = if let Some(store) = self.skill_packs.as_ref() {
                    self.renew_skill_pack_io_lease(lease, id, "read").await?;
                    let result = tokio::time::timeout(SKILL_PACK_IO_DEADLINE, store.get(id)).await;
                    self.renew_skill_pack_io_lease(lease, id, "read").await?;
                    match result {
                        Err(_) => {
                            return Err(crate::provision_failure::StartFailure::transient(format!(
                                "this session's stored skills did not finish reading within {}s",
                                SKILL_PACK_IO_DEADLINE.as_secs()
                            )))
                        }
                        Ok(result) => match result {
                            Ok(Some(packs)) => packs,
                            Ok(None) => {
                                self.refetch_legacy_skill_packs(&record, legacy_wanted)
                                    .await
                            }
                            Err(error) => {
                                return Err(crate::provision_failure::StartFailure::transient(
                                    format!(
                                        "this session's stored skills could not be read: {error}"
                                    ),
                                ))
                            }
                        },
                    }
                } else {
                    self.refetch_legacy_skill_packs(&record, legacy_wanted)
                        .await
                };
                self.validate_prepared_skill_packs(&packs)?;

                // Ordinary recovery stages the fallback before provider create.
                // A handover stages it atomically with the Rotating boundary so
                // a failed preflight leaves H1's legacy grant/custody coherent.
                if stage_legacy {
                    self.persist_skill_pack_state_durably(
                        &mut record,
                        SkillPackState::Staged(packs.clone()),
                    )
                    .await
                    .map_err(|error| {
                        crate::provision_failure::StartFailure::transient(format!(
                            "legacy skill bytes could not be staged for recovery: {error}"
                        ))
                    })?;
                }
                let stored = self.store_staged_skill_packs(id, &packs, lease).await?;
                (packs, stored)
            }
            SkillPackState::StagedSummary => {
                return Err(crate::provision_failure::StartFailure::transient(
                    "the session store returned a projected skill custody summary for an exact read"
                        .into(),
                ))
            }
            SkillPackState::CleanupPending | SkillPackState::Cleaned => return Ok(None),
        };

        let resolved = PreparedSkillPacks::new(packs, externally_stored);
        *prepared = Some(resolved.clone());
        Ok(Some(resolved))
    }

    fn validate_prepared_skill_packs(
        &self,
        packs: &[crate::store::SkillPack],
    ) -> Result<(), crate::provision_failure::StartFailure> {
        crate::store::validate_skill_packs(packs).map_err(|error| {
            crate::provision_failure::StartFailure::transient(format!(
                "this session's skill packs are invalid: {error}"
            ))
        })?;
        if let Some(store) = self.skill_packs.as_ref() {
            store.validate(packs).map_err(|error| {
                crate::provision_failure::StartFailure::transient(format!(
                    "this session's skill packs cannot be represented by its store: {error}"
                ))
            })?;
        }
        Ok(())
    }

    async fn read_stored_skill_packs(
        &self,
        id: &SessionId,
        lease: Option<&SessionLease>,
    ) -> Result<Vec<crate::store::SkillPack>, crate::provision_failure::StartFailure> {
        let store = self.skill_packs.as_ref().ok_or_else(|| {
            crate::provision_failure::StartFailure::transient(
                "this session's skill store is not configured".into(),
            )
        })?;
        self.renew_skill_pack_io_lease(lease, id, "read").await?;
        let result = tokio::time::timeout(SKILL_PACK_IO_DEADLINE, store.get(id)).await;
        self.renew_skill_pack_io_lease(lease, id, "read").await?;
        result
            .map_err(|_| {
                crate::provision_failure::StartFailure::transient(format!(
                    "this session's stored skills did not finish reading within {}s",
                    SKILL_PACK_IO_DEADLINE.as_secs()
                ))
            })?
            .map_err(|error| {
                crate::provision_failure::StartFailure::transient(format!(
                    "this session's stored skills could not be read: {error}"
                ))
            })?
            .ok_or_else(|| {
                crate::provision_failure::StartFailure::transient(
                    "this session's recorded skill packs are missing".into(),
                )
            })
    }

    async fn store_staged_skill_packs(
        &self,
        id: &SessionId,
        packs: &[crate::store::SkillPack],
        lease: Option<&SessionLease>,
    ) -> Result<bool, crate::provision_failure::StartFailure> {
        let Some(store) = self.skill_packs.as_ref() else {
            return Ok(false);
        };
        self.renew_skill_pack_io_lease(lease, id, "write").await?;
        let result = tokio::time::timeout(SKILL_PACK_IO_DEADLINE, store.put(id, packs)).await;
        self.renew_skill_pack_io_lease(lease, id, "write").await?;
        result
            .map_err(|_| {
                crate::provision_failure::StartFailure::transient(format!(
                    "this session's skills did not finish storing within {}s",
                    SKILL_PACK_IO_DEADLINE.as_secs()
                ))
            })?
            .map_err(|error| {
                crate::provision_failure::StartFailure::transient(format!(
                    "this session's skills could not be stored for recovery: {error}"
                ))
            })?;
        Ok(true)
    }

    async fn renew_skill_pack_io_lease(
        &self,
        lease: Option<&SessionLease>,
        id: &SessionId,
        operation: &str,
    ) -> Result<(), crate::provision_failure::StartFailure> {
        self.renew_runtime_lease(lease).await.map_err(|error| {
            crate::provision_failure::StartFailure::transient(format!(
                "runtime authority expired before skill pack {operation} for {id}: {error}"
            ))
        })
    }

    async fn refetch_legacy_skill_packs(
        &self,
        record: &crate::store::SessionRecord,
        legacy_wanted: &[String],
    ) -> Vec<crate::store::SkillPack> {
        let wanted = record.session.granted_skills.as_ref().map_or_else(
            || {
                legacy_wanted
                    .iter()
                    .cloned()
                    .map(crate::environment::SkillId::new)
                    .collect::<Vec<_>>()
            },
            |granted| granted.iter().map(|skill| skill.id.clone()).collect(),
        );
        if wanted.is_empty() {
            return Vec::new();
        }
        let Some(source) = self.skills.as_ref() else {
            tracing::warn!(
                session_id = %record.session.id,
                "legacy skill packs are absent and no registry is configured; carrying the session forward without skills"
            );
            return Vec::new();
        };
        match source.packs_for(None, &wanted).await {
            Ok(packs) => packs,
            Err(why) => {
                tracing::warn!(
                    session_id = %record.session.id,
                    %why,
                    "legacy skill packs could not be refetched; carrying the session forward without skills"
                );
                Vec::new()
            }
        }
    }

    fn require_matching_skill_grant(
        &self,
        record: &crate::store::SessionRecord,
        packs: &[crate::store::SkillPack],
    ) -> Result<(), crate::provision_failure::StartFailure> {
        let installed = Self::granted_skills_for(packs);
        match record.session.granted_skills.as_deref() {
            Some(granted) if granted == installed.as_slice() => Ok(()),
            Some(_) => Err(crate::provision_failure::StartFailure::transient(
                "the stored skill packs do not match this session's frozen grant".into(),
            )),
            None => Err(crate::provision_failure::StartFailure::transient(
                "this session has skill bytes but no frozen grant".into(),
            )),
        }
    }

    /// Drive one terminal session through durable cleanup intent and acknowledgement.
    ///
    /// The row remains the owner until deterministic object deletion succeeds.
    /// Every state write owns only `skill_packs`, so unrelated event history can
    /// advance without reopening custody or losing the cleanup acknowledgement.
    pub(crate) async fn finish_skill_pack_cleanup(
        &self,
        record: &mut crate::store::SessionRecord,
        lease: Option<&SessionLease>,
    ) -> Result<(), StoreError> {
        use crate::store::SkillPackState;

        let id = record.session.id.clone();
        loop {
            self.renew_runtime_lease(lease).await?;
            if !record.session.status.is_terminal() {
                return Err(StoreError::Malformed(format!(
                    "skill pack cleanup requires terminal session {id}"
                )));
            }
            match &record.skill_packs {
                SkillPackState::Cleaned => {
                    self.synchronize_live_skill_pack_custody(record).await;
                    return Ok(());
                }
                // An explicitly empty staged set has no object when this
                // deployment has never had a pack store. Avoid making ordinary
                // no-skill sessions undeletable while preserving every state
                // that could still name external bytes.
                SkillPackState::Staged(packs) if packs.is_empty() && self.skill_packs.is_none() => {
                    if self
                        .advance_skill_pack_cleanup_state(record, SkillPackState::Cleaned)
                        .await?
                    {
                        self.synchronize_live_skill_pack_custody(record).await;
                        return Ok(());
                    }
                    continue;
                }
                SkillPackState::LegacyHandover {
                    stored_digest: None,
                    ..
                } if self.skill_packs.is_none() => {
                    if self
                        .advance_skill_pack_cleanup_state(record, SkillPackState::Cleaned)
                        .await?
                    {
                        self.synchronize_live_skill_pack_custody(record).await;
                        return Ok(());
                    }
                    continue;
                }
                SkillPackState::CleanupPending => {}
                SkillPackState::StagedSummary => {
                    return Err(StoreError::Malformed(format!(
                        "skill pack cleanup for {id} received a projected custody summary from an exact read"
                    )))
                }
                SkillPackState::Legacy
                | SkillPackState::LegacyHandover { .. }
                | SkillPackState::Staged(_)
                | SkillPackState::Stored { .. } => {
                    if self.skill_packs.is_none() {
                        return Err(StoreError::Unavailable(format!(
                            "skill pack cleanup for {id} requires a configured skill pack store"
                        )));
                    }
                    if !self
                        .advance_skill_pack_cleanup_state(record, SkillPackState::CleanupPending)
                        .await?
                    {
                        continue;
                    }
                    self.synchronize_live_skill_pack_custody(record).await;
                }
            }

            let store = self.skill_packs.as_ref().ok_or_else(|| {
                StoreError::Unavailable(format!(
                    "skill pack cleanup for {id} is pending but no skill pack store is configured"
                ))
            })?;
            self.renew_runtime_lease(lease).await?;
            let deletion = tokio::time::timeout(SKILL_PACK_IO_DEADLINE, store.delete(&id)).await;
            // If deletion outlives coordinated authority, leave the durable
            // Pending receipt for a fresh lease holder to replay idempotently.
            self.renew_runtime_lease(lease).await?;
            deletion.map_err(|_| {
                StoreError::Unavailable(format!(
                    "skill pack cleanup for {id} did not finish within {}s",
                    SKILL_PACK_IO_DEADLINE.as_secs()
                ))
            })??;
            if self
                .advance_skill_pack_cleanup_state(record, SkillPackState::Cleaned)
                .await?
            {
                self.synchronize_live_skill_pack_custody(record).await;
                tracing::info!(session_id = %id, "session skill custody cleaned");
                return Ok(());
            }
        }
    }

    /// Persist one cleanup state, reloading rather than guessing after a race.
    async fn advance_skill_pack_cleanup_state(
        &self,
        record: &mut crate::store::SessionRecord,
        state: crate::store::SkillPackState,
    ) -> Result<bool, StoreError> {
        let id = record.session.id.clone();
        match self.persist_skill_pack_state_durably(record, state).await {
            Ok(()) => Ok(true),
            Err(StoreError::RevisionConflict { .. } | StoreError::SessionTerminated { .. }) => {
                *record = self
                    .store
                    .get(&id)
                    .await?
                    .ok_or(StoreError::SessionDeleted { id })?;
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// Acquire fresh cross-replica and local authority for terminal skill cleanup.
    ///
    /// The acquisition and release live in one method so every failure after a
    /// lease is granted still releases it. Reconciliation uses `Reclaim`; an
    /// owner deletion uses `Lifecycle` and waits for this same acknowledgement
    /// before it is allowed to erase the row.
    pub(crate) async fn finish_terminal_skill_pack_cleanup_with_authority(
        &self,
        id: &SessionId,
        operation: SessionOperation,
    ) -> Result<crate::store::SessionRecord, StoreError> {
        let initial = self
            .store
            .get(id)
            .await?
            .ok_or_else(|| StoreError::SessionDeleted { id: id.clone() })?;
        if !initial.session.status.is_terminal() {
            return Err(StoreError::Malformed(format!(
                "skill pack cleanup requires terminal session {id}"
            )));
        }
        if initial.skill_packs == crate::store::SkillPackState::Cleaned {
            self.synchronize_live_skill_pack_custody(&initial).await;
            return Ok(initial);
        }

        let lease = self.acquire_runtime_lease_at(&initial, operation).await?;
        let lifecycle = self.lock_lifecycle(id).await;
        let result = async {
            self.renew_runtime_lease(lease.as_ref()).await?;
            let mut record = self
                .store
                .get(id)
                .await?
                .ok_or_else(|| StoreError::SessionDeleted { id: id.clone() })?;
            self.finish_skill_pack_cleanup(&mut record, lease.as_ref())
                .await?;
            Ok(record)
        }
        .await;
        drop(lifecycle);

        let release = match lease.as_ref() {
            Some(held) => self.store.release_session_lease(held).await,
            None => Ok(()),
        };
        match (result, release) {
            (Err(error), release) => {
                if let Err(release_error) = release {
                    tracing::debug!(
                        session_id = %id,
                        %release_error,
                        "skill cleanup lease ended before explicit release"
                    );
                }
                Err(error)
            }
            (Ok(record), Ok(())) => Ok(record),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    /// Continue compatibility cleanup outside the retained lane operation.
    ///
    /// A retained continuation cannot await a new write on its own lane. This
    /// scheduler lets that continuation return first, then reacquires the local
    /// lifecycle gate and drives the durable saga from a fresh terminal row.
    pub(crate) fn finish_skill_pack_cleanup_after_commit(self: &Arc<Self>, id: SessionId) {
        let service = Arc::clone(self);
        crate::log::spawn_retained_work(async move {
            if let Err(error) = service
                .finish_terminal_skill_pack_cleanup_with_authority(&id, SessionOperation::Reclaim)
                .await
            {
                tracing::error!(
                    session_id = %id,
                    %error,
                    "session skill cleanup remains durable for reconciliation"
                );
            }
        });
    }

    async fn synchronize_live_skill_pack_custody(&self, record: &crate::store::SessionRecord) {
        if let Some(entry) = self
            .entries
            .write()
            .await
            .get_mut(&record.session.id)
            .filter(|entry| entry.session.status.is_terminal())
        {
            entry.skill_packs.clone_from(&record.skill_packs);
        }
    }

    /// Provision a runtime for an existing session and attach its terminal.
    pub(crate) async fn bring_up(
        self: &Arc<Self>,
        session: Session,
        spec: SessionSpec,
        adapter: Arc<dyn KernelAdapter>,
        geometry: TerminalGeometry,
        restore: Option<Vec<u8>>,
        context: BringUpContext,
    ) {
        self.bring_up_with_lease(session, adapter, spec, geometry, restore, context)
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
        context: BringUpContext,
    ) {
        let BringUpContext {
            outcome,
            mut lease,
            handover,
            mut prepared_skills,
            rotation_claim,
        } = context;
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
        // Kept for the common failure path below. Abandoning here would run
        // before ordinary provisioning acquires its durable runtime lease and
        // could turn a coordinated authority into handle-only deletion.
        let (mut credential_failure, credential_available) = match self
            .credential_for(
                &session.id,
                &session.owner,
                &services,
                spec.caller_credential.as_ref(),
            )
            .await
        {
            // This call establishes durable custody before a provider runtime
            // is created. Its value is intentionally discarded: exact
            // selection happens only after the target generation exists.
            Ok(credential) => (None, credential.is_some()),
            Err(error) => {
                tracing::error!(
                    session_id = %id,
                    error = %error,
                    "could not establish durable credential custody"
                );
                (
                    Some(crate::provision_failure::StartFailure::transient(
                        "the durable credential store is unavailable".to_string(),
                    )),
                    false,
                )
            }
        };
        if credential_failure.is_none() && !credential_available {
            tracing::warn!(
                session_id = %id,
                "no credential could be minted for this session; it will start unable to act \
                 on its owner's behalf until the top-up sweep supplies one"
            );
        }
        let operation = if session.status.is_rotating() || rotation_claim.is_some() {
            SessionOperation::Handover
        } else {
            SessionOperation::Provision
        };
        let publication_token = format!("publication_{}", uuid::Uuid::new_v4().simple());
        let start = crate::provision::RuntimeStartContext {
            geometry,
            restore: restore.as_ref(),
            platform_briefing: &platform_briefing,
            handover: handover.as_ref(),
            publication_token: &publication_token,
        };
        let mut coordination_error = false;
        let started = if lease.is_none() {
            match self.acquire_runtime_lease(&id, operation).await {
                Ok(acquired) => {
                    lease = acquired;
                    match credential_failure.take() {
                        Some(failure) => Ok(Err(failure)),
                        None => {
                            tokio::time::timeout(
                                PROVISION_DEADLINE,
                                self.start_with_retries(
                                    &session,
                                    &spec,
                                    &adapter,
                                    start,
                                    &mut lease,
                                    &mut prepared_skills,
                                ),
                            )
                            .await
                        }
                    }
                }
                Err(error) => {
                    coordination_error = true;
                    Ok(Err(crate::provision_failure::StartFailure::transient(
                        format!("durable runtime authority could not be acquired: {error}"),
                    )))
                }
            }
        } else if let Some(failure) = credential_failure {
            Ok(Err(failure))
        } else {
            tokio::time::timeout(
                PROVISION_DEADLINE,
                self.start_with_retries(
                    &session,
                    &spec,
                    &adapter,
                    start,
                    &mut lease,
                    &mut prepared_skills,
                ),
            )
            .await
        };

        let failed = !matches!(&started, Ok(Ok(())));
        if failed {
            self.remove_incomplete_runtime_entry(&id, &publication_token)
                .await;
        }
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
                Ok(Err(failure)) => {
                    self.abandon(&id, &adapter, failure, outcome, rotation_claim.as_deref())
                        .await
                }
                Err(_) => {
                    let failure = crate::provision_failure::StartFailure::transient(format!(
                        "the runtime did not become usable within {}s",
                        PROVISION_DEADLINE.as_secs()
                    ));
                    self.abandon(&id, &adapter, &failure, outcome, rotation_claim.as_deref())
                        .await
                }
                Ok(Ok(())) => unreachable!("only a failed start is abandoned"),
            };
            match abandonment {
                Err(error) => tracing::error!(
                    session_id = %id,
                    %error,
                    "cannot record the coordinated runtime abandonment; leaving its debt intact"
                ),
                Ok(Some(mut record)) => {
                    let mut advanced = false;
                    if let Some(held) = lease.as_mut() {
                        let boundary = SessionBoundary::for_record(
                            &record,
                            held.boundary.authority.generation,
                        );
                        if let Err(error) = self.advance_runtime_lease(held, &boundary).await {
                            tracing::warn!(
                                session_id = %id,
                                %error,
                                "the abandonment committed but its runtime lease could not advance"
                            );
                        } else {
                            advanced = true;
                        }
                    }
                    if advanced && record.session.status.is_terminal() {
                        if let Some(held) = lease.as_ref() {
                            if let Err(error) = self.renew_runtime_lease(Some(held)).await {
                                tracing::warn!(
                                    session_id = %id,
                                    %error,
                                    "the abandonment lease expired before skill cleanup"
                                );
                            } else if let Err(error) = self
                                .finish_skill_pack_cleanup(&mut record, Some(held))
                                .await
                            {
                                tracing::error!(
                                    session_id = %id,
                                    %error,
                                    "abandoned session skill cleanup remains pending"
                                );
                            }
                        }
                    }
                }
                Ok(None) => {}
            };
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

        // `start_runtime` returns only after the exact replacement entry and
        // conversation link are locally published. Clear the durable claim
        // while the provider-operation lease still excludes another replica.
        // A failed final write is not a failed runtime start: leaving the claim
        // set keeps every interaction fenced, and readoption/reconciliation
        // repairs it without provisioning another sandbox.
        if matches!(&started, Ok(Ok(()))) {
            if let Some(claim) = rotation_claim.as_deref() {
                match self.finalize_rotation_publication(&id, claim).await {
                    Ok(true) => {}
                    Ok(false) => tracing::warn!(
                        session_id = %id,
                        "the replacement runtime was published after its rotation claim changed"
                    ),
                    Err(error) => tracing::warn!(
                        session_id = %id,
                        %error,
                        "the published replacement remains interaction-fenced until reconciliation clears its rotation claim"
                    ),
                }
            }
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
                match self
                    .abandon(&id, &adapter, &failure, outcome, rotation_claim.as_deref())
                    .await
                {
                    Ok(Some(record)) if record.session.status.is_terminal() => {
                        self.finish_skill_pack_cleanup_after_commit(id.clone());
                    }
                    Ok(_) => {}
                    Err(error) => tracing::error!(
                        session_id = %id,
                        %error,
                        "cannot record the runtime abandonment; leaving it intact"
                    ),
                }
            }
            Err(_) if coordinated_failure => {
                tracing::error!(
                    session_id = %id,
                    "coordinated provisioning timed out; durable authority and cleanup debt remain for recovery"
                );
            }
            Err(_) => {
                match self
                    .abandon(
                        &id,
                        &adapter,
                        &crate::provision_failure::StartFailure::unverified_attempt(format!(
                            "the runtime did not become usable within {}s",
                            PROVISION_DEADLINE.as_secs()
                        )),
                        outcome,
                        rotation_claim.as_deref(),
                    )
                    .await
                {
                    Ok(Some(record)) if record.session.status.is_terminal() => {
                        self.finish_skill_pack_cleanup_after_commit(id.clone());
                    }
                    Ok(_) => {}
                    Err(error) => tracing::error!(
                        session_id = %id,
                        %error,
                        "cannot record the timed-out runtime abandonment; leaving it intact"
                    ),
                }
            }
        }
        // Released last, and on every path: while the claim is held the
        // controller leaves this session alone, so failing to release it would
        // strand a session nothing is allowed to recover.
        self.finish_start(&id).await;
    }

    /// Clear an exact rotation claim only after its authoritative runtime entry
    /// has been published by this process.
    ///
    /// Runtime identity is checked against both the durable record and, where
    /// coordination is enabled, its monotonic generation. A stale outgoing
    /// entry in the durable `Ready` gap therefore cannot clear the claim for
    /// the candidate that has just replaced it.
    pub(crate) async fn finalize_rotation_publication(
        &self,
        id: &SessionId,
        expected_claim: &str,
    ) -> Result<bool, StoreError> {
        // Publication finalization can race an owner lifecycle operation after
        // that operation's durable write but before its retained continuation
        // consumes the local entry. Serialize the whole check/remove/claim-clear
        // decision with lifecycle and input delivery so a successful stop or
        // termination remains successful and owns its exact local teardown.
        let _lifecycle = self.lock_lifecycle(id).await;
        let _input_fence = self.input_gates.gate(id).write_owned().await;
        let published = {
            let entries = self.entries.read().await;
            entries.get(id).and_then(|entry| {
                if entry.session.status.is_rotating() || !entry.publication_complete {
                    return None;
                }
                entry.runtime.as_ref().map(|runtime| {
                    (
                        runtime.0.clone(),
                        entry.runtime_generation,
                        Arc::clone(&entry.screen),
                    )
                })
            })
        };
        let Some((runtime_id, runtime_generation, screen)) = published else {
            return Ok(false);
        };

        match self.store.runtime_authority(id).await {
            Ok(Some(authority))
                if authority.runtime_id.as_deref() == Some(runtime_id.as_str())
                    && authority.generation == runtime_generation => {}
            Ok(Some(_)) => {
                self.remove_exact_runtime_entry(id, &runtime_id, runtime_generation, &screen)
                    .await;
                return Ok(false);
            }
            Ok(None) | Err(StoreError::SessionDeleted { .. }) => {
                return Err(StoreError::SessionDeleted { id: id.clone() });
            }
            Err(StoreError::CoordinationDisabled) => {}
            Err(error) => return Err(error),
        }

        let Some(mut current) = self.store.get(id).await? else {
            return Err(StoreError::SessionDeleted { id: id.clone() });
        };
        if current.runtime_id.as_deref() != Some(runtime_id.as_str())
            || current.session.status.is_rotating()
            || !current.session.wants_runtime()
        {
            self.remove_exact_runtime_entry(id, &runtime_id, runtime_generation, &screen)
                .await;
            return Ok(false);
        }
        if current.rotation_claim.is_none() {
            return Ok(true);
        }
        if current.rotation_claim.as_deref() != Some(expected_claim) {
            self.remove_exact_runtime_entry(id, &runtime_id, runtime_generation, &screen)
                .await;
            return Ok(false);
        }
        if current
            .pending_handover
            .as_ref()
            .is_some_and(|pending| pending.claim != expected_claim)
        {
            self.remove_exact_runtime_entry(id, &runtime_id, runtime_generation, &screen)
                .await;
            return Ok(false);
        }

        let claimed = current.clone();
        current.rotation_claim = None;
        current.pending_handover = None;
        self.clear_rotation_claim_durably(&mut current, &claimed)
            .await?;
        Ok(true)
    }

    /// Remove only the local entry whose publication identity was just shown
    /// to be stale. A newer entry may appear while the durable checks await;
    /// matching both handle and generation prevents that successor from being
    /// discarded by the obsolete finalizer.
    async fn remove_exact_runtime_entry(
        &self,
        id: &SessionId,
        runtime_id: &str,
        runtime_generation: u64,
        screen: &Arc<tokio::sync::Mutex<Box<dyn crate::terminal::ScreenModel>>>,
    ) {
        let removed = {
            let mut entries = self.entries.write().await;
            let exact = entries.get(id).is_some_and(|entry| {
                entry.runtime.as_ref().map(|runtime| runtime.0.as_str()) == Some(runtime_id)
                    && entry.runtime_generation == runtime_generation
                    && Arc::ptr_eq(&entry.screen, screen)
            });
            exact.then(|| entries.remove(id)).flatten()
        };
        if let Some(entry) = removed {
            if let Some(attached) = entry.attached.as_ref() {
                let _ = attached
                    .supersede
                    .send(crate::lease::Detached::RuntimeReplaced);
            }
        }
    }

    /// Remove only the incomplete local entry installed by one exact attempt.
    pub(crate) async fn remove_incomplete_runtime_entry(
        &self,
        id: &SessionId,
        publication_token: &str,
    ) {
        let removed = {
            let mut entries = self.entries.write().await;
            let exact = entries.get(id).is_some_and(|entry| {
                !entry.publication_complete && entry.publication_token == publication_token
            });
            exact.then(|| entries.remove(id)).flatten()
        };
        if let Some(entry) = removed {
            if let Some(attached) = entry.attached.as_ref() {
                let _ = attached
                    .supersede
                    .send(crate::lease::Detached::RuntimeReplaced);
            }
        }
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
        if operation == SessionOperation::Interact {
            let Some(request) = self.interaction_lease_request_at(record).await? else {
                return Ok(None);
            };
            return self
                .store
                .acquire_interaction_lease(&request)
                .await
                .map(Some);
        }
        let id = &record.session.id;
        let authority = match self.store.runtime_authority(id).await {
            Ok(Some(authority)) => authority,
            Ok(None) => return Err(StoreError::SessionDeleted { id: id.clone() }),
            Err(StoreError::CoordinationDisabled) => return Ok(None),
            Err(error) => return Err(error),
        };
        let boundary = SessionBoundary::for_record(record, authority.generation);
        let request = SessionLeaseRequest::new(
            id.clone(),
            self.coordination_holder.clone(),
            boundary,
            operation,
            RUNTIME_OPERATION_LEASE_TTL,
        );
        self.store.acquire_session_lease(&request).await.map(Some)
    }

    async fn interaction_lease_request_at(
        &self,
        record: &crate::store::SessionRecord,
    ) -> Result<Option<SessionInteractionLeaseRequest>, StoreError> {
        let id = &record.session.id;
        let authority = match self.store.runtime_authority(id).await {
            Ok(Some(authority)) => authority,
            Ok(None) => return Err(StoreError::SessionDeleted { id: id.clone() }),
            Err(StoreError::CoordinationDisabled) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(Some(SessionInteractionLeaseRequest::new(
            id.clone(),
            self.coordination_holder.clone(),
            SessionBoundary::for_record(record, authority.generation),
            INTERACTION_LEASE_TTL,
        )))
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
        self.run_retained_runtime_interaction_at(
            record,
            move |authority, _interaction| action(authority),
            |_reason, _result, _interaction| {},
        )
        .await
    }

    /// Run an interaction whose authority can move into retained continuation work.
    ///
    /// The action may clone `interaction` into durable continuations, rejection
    /// work, or another service-owned cleanup. Its final owner queues exact
    /// release synchronously. `on_undelivered` receives ownership when the
    /// service cannot publish the result or the caller drops it before polling.
    pub(crate) async fn run_retained_runtime_interaction_at<T, A, F, U>(
        self: &Arc<Self>,
        record: &crate::store::SessionRecord,
        action: A,
        on_undelivered: U,
    ) -> Result<T, CoreError>
    where
        T: Send + 'static,
        A: FnOnce(RuntimeInteractionAuthority, RetainedRuntimeInteraction) -> F + Send + 'static,
        F: Future<Output = Result<T, CoreError>> + Send + 'static,
        U: FnOnce(RuntimeInteractionDeliveryLoss, Result<T, CoreError>, RetainedRuntimeInteraction)
            + Send
            + 'static,
    {
        let mut caller = RuntimeInteractionCallerGuard::new();
        let caller_state = caller.state();
        let (finished, result) = tokio::sync::oneshot::channel();
        let service = Arc::clone(self);
        let record = record.clone();
        tokio::spawn(async move {
            let acquisition = service
                .acquire_interaction_for(&record, &caller_state)
                .await;
            let (lease, acquisition_started, acquisition_completed) = match acquisition {
                Ok(RuntimeInteractionAcquisition::Acquired {
                    lease,
                    started,
                    completed,
                }) => (lease, started, completed),
                Ok(RuntimeInteractionAcquisition::CallerGone) => return,
                Ok(RuntimeInteractionAcquisition::Unknown { report }) => {
                    if let Some(error) = report {
                        let _ = finished.send(Err(error));
                    }
                    drop(action);
                    drop(on_undelivered);
                    return;
                }
                Err(error) => {
                    let _ = finished.send(Err(error));
                    return;
                }
            };

            let boundary = lease.as_ref().map_or_else(
                || SessionBoundary::for_record(&record, 0),
                |lease| lease.boundary.clone(),
            );
            let lease_deadline = lease
                .as_ref()
                .map(|_| acquisition_started + INTERACTION_LEASE_TTL);
            let lease_expiry_bound = lease
                .as_ref()
                .map(|_| acquisition_completed + INTERACTION_LEASE_TTL);
            let authority =
                RuntimeInteractionAuthority::new(boundary, lease_deadline, lease_expiry_bound);
            let interaction = RetainedRuntimeInteraction::new(
                &service,
                record.session.id.clone(),
                lease,
                authority.clone(),
            );
            if !caller_state.register_authority(authority.clone()) {
                drop(action);
                drop(on_undelivered);
                drop(interaction);
                return;
            }
            let action_interaction = interaction.clone();
            let action_result = service
                .run_owned_runtime_interaction(
                    interaction.state.lease.as_ref(),
                    authority,
                    move |authority| action(authority, action_interaction),
                )
                .await;
            let delivery =
                RetainedRuntimeInteractionDelivery::new(action_result, interaction, on_undelivered);
            if let Err(Ok(mut undelivered)) = finished.send(Ok(delivery)) {
                undelivered.report_undelivered(RuntimeInteractionDeliveryLoss::SendFailed);
            }
        });
        let delivery = result.await;
        caller.complete();
        delivery
            .map_err(|_| {
                CoreError::RuntimeUnavailable("this session's interaction task stopped".into())
            })??
            .observed()
    }

    /// Acquire once under service ownership after giving local cleanup one chance.
    async fn acquire_interaction_for(
        self: &Arc<Self>,
        record: &crate::store::SessionRecord,
        caller: &RuntimeInteractionCallerState,
    ) -> Result<RuntimeInteractionAcquisition, CoreError> {
        if record.is_effectively_rotating() {
            return Err(CoreError::SessionRotating);
        }
        let request_started = tokio::time::Instant::now();
        let request = self
            .interaction_lease_request_at(record)
            .await
            .map_err(store_failed)?;
        let Some(request) = request else {
            return Ok(if caller.is_active() {
                RuntimeInteractionAcquisition::Acquired {
                    lease: None,
                    started: request_started,
                    completed: tokio::time::Instant::now(),
                }
            } else {
                RuntimeInteractionAcquisition::CallerGone
            });
        };
        let mut acquisition_guard = RuntimeInteractionAcquisitionGuard::new(self, request.clone());
        let mut resumed_local_cleanup = false;
        loop {
            if !caller.is_active() {
                acquisition_guard.definitive();
                return Ok(RuntimeInteractionAcquisition::CallerGone);
            }
            // This is a conservative lower bound for the store's authoritative
            // acquisition time and is reset if cleanup makes a retry necessary.
            let acquisition_started = tokio::time::Instant::now();
            acquisition_guard.attempted();
            let acquisition = self.store.acquire_interaction_lease(&request);
            tokio::pin!(acquisition);
            let outcome = tokio::select! {
                biased;
                () = caller.stopped() => {
                    return Ok(RuntimeInteractionAcquisition::Unknown { report: None });
                }
                outcome = &mut acquisition => outcome,
            };
            match outcome {
                Ok(lease) => {
                    acquisition_guard.disarm();
                    return Ok(RuntimeInteractionAcquisition::Acquired {
                        lease: Some(lease),
                        started: acquisition_started,
                        completed: tokio::time::Instant::now(),
                    });
                }
                Err(error @ StoreError::Unavailable(_)) => {
                    return Ok(RuntimeInteractionAcquisition::Unknown {
                        report: Some(store_failed(error)),
                    });
                }
                Err(StoreError::SessionInteractionBlocked {
                    operation: SessionOperation::Interact,
                    ..
                }) if !resumed_local_cleanup => {
                    acquisition_guard.definitive();
                    resumed_local_cleanup = true;
                    self.event_logs
                        .drain_retained_cleanups(&record.session.id)
                        .await;
                }
                Err(StoreError::SessionInteractionBlocked {
                    operation: SessionOperation::Handover,
                    ..
                }) => {
                    acquisition_guard.disarm();
                    return Err(CoreError::SessionRotating);
                }
                Err(StoreError::SessionAuthorityChanged { .. }) => {
                    acquisition_guard.disarm();
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
                        Some(current) if current.is_effectively_rotating() => {
                            Err(CoreError::SessionRotating)
                        }
                        Some(_) => Err(CoreError::RuntimeUnavailable(
                            "the session runtime changed before interaction delivery".into(),
                        )),
                    };
                }
                Err(error) => {
                    acquisition_guard.disarm();
                    return Err(store_failed(error));
                }
            }
        }
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
                    let mut renewal = RuntimeInteractionRenewalGuard::new(authority.clone());
                    let outcome = self
                        .store
                        .renew_interaction_lease(lease, INTERACTION_LEASE_TTL)
                        .await;
                    match outcome {
                        Ok(()) => {
                            renewal.known();
                            authority.extend(
                                renewal_started + INTERACTION_LEASE_TTL,
                                tokio::time::Instant::now() + INTERACTION_LEASE_TTL,
                            );
                            next_renewal =
                                renewal_started + INTERACTION_LEASE_RENEW_INTERVAL;
                        }
                        Err(StoreError::Unavailable(error)) => {
                            renewal.unknown();
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
                            renewal.known();
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
        let marker = Event {
            session_id: id.clone(),
            seq: 0,
            kind: EventKind::RuntimeIncarnationStarted,
            appended_at: String::new(),
        };
        if let Err(e) = self.persist_durably(&mut record, vec![marker]).await {
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
