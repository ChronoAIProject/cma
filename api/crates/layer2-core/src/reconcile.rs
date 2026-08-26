//! Driving what is running towards what is recorded.
//!
//! The record is the only authority on which sessions should have a runtime.
//! Nothing else — not this process's memory, not what the provider happens to
//! be running — decides that, because everything else is lost or wrong after a
//! restart while the record is not.
//!
//! So this compares the two and closes the gap in both directions, on boot and
//! on a timer:
//!
//! A session that should have a runtime and does. Nothing to do, unless this
//! process is not the one holding the attachment — then it takes it back, which
//! is what makes a deployment invisible to someone typing into a session.
//!
//! A session that should have a runtime and has none. Its sandbox was lost, or
//! the bring-up that was starting it died with an earlier process. It is
//! started again rather than failed: the owner asked for a session and has not
//! said to stop, and nothing about this process dying is their problem.
//!
//! A runtime no such session refers to. Torn down. This covers the sandbox
//! created a moment before the process recording it was killed, and the one
//! left behind by a session that has since been stopped.
//!
//! The asymmetry is deliberate. Failing to start something is visible and
//! recoverable; tearing down someone's live work is neither. So a runtime is
//! only reclaimed when the record positively says it should not exist, never
//! merely because this process has not heard of it.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{RuntimeHandle, RuntimeIdentity};

use crate::coordination::{
    RuntimeAuthority, RuntimeCleanup, RuntimeCleanupReason, SessionBoundary, SessionLease,
    SessionLeaseRequest, SessionOperation,
};
use crate::event::{Event, EventKind};
use crate::ids::KernelId;
use crate::service::SessionService;
use crate::session::{RecoveryState, RuntimeLossCause, SessionSpec, SessionStatus};
use crate::store::{SessionRecord, StoreError};

const RECLAIM_LEASE_TTL: Duration = Duration::from_secs(30);

/// What a sweep did, for logging and for tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reclaimed {
    /// Exact generation-tagged cleanup debts successfully torn down.
    pub orphaned_runtimes: usize,
    /// Live runtimes this process took back over.
    pub readopted_sessions: usize,
    /// Sessions given a new runtime because theirs was gone.
    pub reprovisioned_sessions: usize,
    /// Runtimes found gone, and recorded as gone in their session's log.
    ///
    /// Counted rather than only logged: one loss is a bad afternoon and forty
    /// in one sweep is a provider outage, and both are worth one clear number.
    pub lost_runtimes: usize,
    /// Sessions carried onto the current runtime version.
    pub rotated_sessions: usize,
    /// Sessions left exactly as they were, because nothing could be confirmed.
    ///
    /// Counted rather than merely logged: a sweep that defers every session is
    /// a provider outage, and one that defers the same session forever is a
    /// sandbox that answers nobody. Both are worth seeing in one number.
    pub deferred_sessions: usize,
}

/// What the provider says is running, for one kernel.
///
/// The `trusted` flag is the important part. An enumeration that failed is not
/// evidence that nothing is running, and treating it as such would tear down
/// every live session the moment the provider had a bad minute.
struct RuntimeCensus {
    /// Runtime ids the provider reports as still **serving**.
    ///
    /// Serving, not merely existing, and the distinction is the whole reason
    /// this field changed meaning. A sandbox whose container has exited still
    /// exists and is still listed, so a set of everything present reads a
    /// corpse as a live runtime — which is how a session came to report
    /// `running` for nineteen minutes after the kernel had killed its
    /// container.
    ///
    /// Cleanup debt is processed separately. Provider census rows remain
    /// diagnostic input and never authorise handle-only deletion.
    serving: HashSet<String>,
    /// Provider-visible generation for every listed runtime.
    generations: HashMap<String, layer3_adapter::RuntimeGeneration>,
    /// Why a runtime that is no longer serving stopped, where one was named.
    ///
    /// Populated only for a listing that reports itself not serving. A healthy
    /// sandbox can carry a provider reason too, and reading it then would
    /// invent a death for every live runtime in the list.
    stopped_because: HashMap<String, layer3_adapter::RuntimeLossReason>,
    /// Runtime id to the session it says it belongs to.
    owners: HashMap<String, Option<String>>,
    trusted: bool,
}

impl SessionService {
    /// Compare the provider's runtimes against the record, and settle both.
    pub async fn reconcile(self: &Arc<Self>) -> Reclaimed {
        let mut reclaimed = Reclaimed::default();

        let records = match self.store.scan_all().await {
            Ok(r) => r,
            Err(e) => {
                // Without the record there is nothing to compare against, and
                // guessing here would tear down live work.
                tracing::error!(error = %e, "reconciliation skipped: cannot read sessions");
                return reclaimed;
            }
        };

        let census = self.take_census().await;

        for record in &records {
            self.reclaim_cleanup_debts(record, &mut reclaimed).await;
        }

        // Only these are entitled to a runtime. A stopped session is not — that
        // is what stopping means — so its sandbox is reclaimed below.
        let entitled: HashSet<String> = records
            .iter()
            .filter(|r| r.session.status.wants_runtime())
            .map(|r| r.session.id.0.clone())
            .collect();

        for view in census.values() {
            if !view.trusted {
                continue;
            }
            for (runtime, session) in &view.owners {
                if session.as_deref().is_some_and(|s| entitled.contains(s)) {
                    continue;
                }
                let generation = view.generations.get(runtime).copied();
                tracing::warn!(
                    runtime = %runtime,
                    ?generation,
                    session = session.as_deref().unwrap_or("<none>"),
                    "runtime is unaccounted but has no exact durable cleanup debt; deferring reclamation"
                );
            }
        }

        for record in records {
            if !record.session.status.wants_runtime() {
                continue;
            }
            self.settle_session(record, &census, &mut reclaimed).await;
        }

        if reclaimed != Reclaimed::default() {
            tracing::info!(
                orphaned_runtimes = reclaimed.orphaned_runtimes,
                readopted_sessions = reclaimed.readopted_sessions,
                reprovisioned_sessions = reclaimed.reprovisioned_sessions,
                lost_runtimes = reclaimed.lost_runtimes,
                rotated_sessions = reclaimed.rotated_sessions,
                "reconciliation moved sessions towards their recorded state"
            );
        }
        reclaimed
    }

    /// Ask every provider what it is running.
    async fn take_census(&self) -> HashMap<KernelId, RuntimeCensus> {
        let mut census = HashMap::new();
        for (kernel, adapter) in &self.adapters {
            let view = match adapter.list_runtimes().await {
                Ok(runtimes) => {
                    let mut owners = HashMap::new();
                    let mut generations = HashMap::new();
                    let mut serving = HashSet::new();
                    let mut stopped_because = HashMap::new();
                    for listing in runtimes {
                        generations.insert(listing.handle.0.clone(), listing.generation);
                        // A runtime this control plane did not create is not
                        // its business. It still counts as alive — a session of
                        // ours pointing at it is not lost — but it is never
                        // reclaimed, because the only thing that knows whether
                        // it should exist is whoever made it.
                        if listing.serving {
                            serving.insert(listing.handle.0.clone());
                        } else if let Some(why) = listing.lost_because {
                            stopped_because.insert(listing.handle.0.clone(), why);
                        }
                        // A runtime this control plane did not create is never
                        // reclaimed, so it is left out of `owners` — but it is
                        // still counted above as serving, because a session of
                        // ours pointing at it is not lost.
                        if !self.may_judge(listing.created_by.as_deref()) {
                            continue;
                        }
                        owners.insert(listing.handle.0, listing.session);
                    }
                    RuntimeCensus {
                        serving,
                        generations,
                        stopped_because,
                        owners,
                        trusted: true,
                    }
                }
                Err(e) => {
                    // Not knowing is not the same as nothing being there.
                    tracing::warn!(kernel = %kernel, error = %e, "cannot enumerate runtimes");
                    RuntimeCensus {
                        serving: HashSet::new(),
                        generations: HashMap::new(),
                        stopped_because: HashMap::new(),
                        owners: HashMap::new(),
                        trusted: false,
                    }
                }
            };
            census.insert(kernel.clone(), view);
        }
        census
    }

    /// Whether a runtime carrying this creator is this control plane's to judge.
    ///
    /// Its own, always. One carrying no identity only if this deployment owns
    /// the provider — otherwise a second control plane sharing the key would
    /// tear down every session that predates identities being recorded.
    fn may_judge(&self, created_by: Option<&str>) -> bool {
        match (created_by, self.identity.as_deref()) {
            (Some(theirs), Some(ours)) => theirs == ours,
            (Some(_), None) => false,
            (None, _) => self.adopts_untagged,
        }
    }

    /// Reclaim only runtimes a durable transaction named exactly.
    ///
    /// Census rows are diagnostic input, never deletion authority. A candidate
    /// or retired runtime becomes deletable only after the store records its
    /// handle and generation as debt, and a fresh exclusive lease proves the
    /// session still has a different authority at the provider-mutation edge.
    async fn reclaim_cleanup_debts(&self, observed: &SessionRecord, reclaimed: &mut Reclaimed) {
        let debts = match self.store.runtime_cleanup_debts(&observed.session.id).await {
            Ok(debts) => debts,
            Err(StoreError::CoordinationDisabled) => return,
            Err(error) => {
                tracing::warn!(
                    session_id = %observed.session.id,
                    %error,
                    "cannot read runtime cleanup debt; deferring reclamation"
                );
                reclaimed.deferred_sessions += 1;
                return;
            }
        };
        if debts.is_empty() {
            return;
        }
        let legacy_cleanup_authorized = if debts.iter().any(|cleanup| cleanup.generation == 0) {
            match self.store.legacy_runtime_cleanup_authorized().await {
                Ok(authorized) => authorized,
                Err(error) => {
                    tracing::warn!(
                        session_id = %observed.session.id,
                        %error,
                        "cannot prove legacy writers are fenced; legacy cleanup debt will remain"
                    );
                    false
                }
            }
        } else {
            false
        };
        let Some(adapter) = self.adapters.get(&observed.session.kernel) else {
            tracing::warn!(
                session_id = %observed.session.id,
                kernel = %observed.session.kernel,
                "cleanup debt names a runtime adapter this deployment does not have"
            );
            reclaimed.deferred_sessions += 1;
            return;
        };

        let Some((record, lease)) = self.acquire_reclaim_lease(&observed.session.id).await else {
            reclaimed.deferred_sessions += 1;
            return;
        };
        if record.session.kernel != observed.session.kernel {
            tracing::warn!(
                session_id = %observed.session.id,
                observed_kernel = %observed.session.kernel,
                current_kernel = %record.session.kernel,
                "session runtime adapter changed while cleanup was being claimed"
            );
            let _ = self.store.release_session_lease(&lease).await;
            reclaimed.deferred_sessions += 1;
            return;
        }

        // Retirement is the only durable copy of the old runtime id after an
        // atomic authority clear. If this process died before recording the
        // loss, recovery still needs that exact id for the RuntimeLost event.
        // Leave this one debt in place until settlement below has recorded the
        // loss; all other debts remain independently reclaimable here.
        let retains_unrecorded_loss = record.session.status.wants_runtime()
            && record.runtime_id.is_none()
            && record.session.status != SessionStatus::Provisioning
            && record.session.recovery_state != RecoveryState::Recovering;

        for cleanup in debts {
            if cleanup.session_id != lease.session_id {
                tracing::error!(
                    leased_session_id = %lease.session_id,
                    cleanup_session_id = %cleanup.session_id,
                    runtime = %cleanup.runtime_id,
                    "store returned cleanup debt outside the leased session; refusing teardown"
                );
                continue;
            }
            if retains_unrecorded_loss
                && cleanup.reason == RuntimeCleanupReason::RetiredAuthority
                && cleanup.generation == lease.boundary.authority.generation
            {
                tracing::debug!(
                    session_id = %cleanup.session_id,
                    runtime = %cleanup.runtime_id,
                    generation = cleanup.generation,
                    "retaining retired runtime identity until its loss is recorded"
                );
                continue;
            }
            if cleanup.generation == 0 && !legacy_cleanup_authorized {
                tracing::warn!(
                    session_id = %cleanup.session_id,
                    runtime = %cleanup.runtime_id,
                    "legacy cleanup requires the monotonic old-writer-drain proof; debt remains"
                );
                continue;
            }
            if lease.boundary.authority.generation == cleanup.generation
                && lease.boundary.authority.runtime_id.as_deref()
                    == Some(cleanup.runtime_id.as_str())
            {
                tracing::error!(
                    session_id = %cleanup.session_id,
                    runtime = %cleanup.runtime_id,
                    generation = cleanup.generation,
                    "cleanup debt names current authority; refusing teardown"
                );
                continue;
            }
            if let Err(error) = self
                .store
                .renew_session_lease(&lease, RECLAIM_LEASE_TTL)
                .await
            {
                tracing::warn!(
                    session_id = %cleanup.session_id,
                    %error,
                    "cleanup lease was lost before provider mutation"
                );
                break;
            }
            let identity = RuntimeIdentity {
                handle: RuntimeHandle(cleanup.runtime_id.clone()),
                session: cleanup.session_id.0.clone(),
                generation: NonZeroU64::new(cleanup.generation),
            };
            if let Err(error) = adapter.terminate_if_identity(&identity).await {
                tracing::warn!(
                    session_id = %cleanup.session_id,
                    runtime = %cleanup.runtime_id,
                    generation = cleanup.generation,
                    %error,
                    "conditional runtime cleanup was refused; debt remains retryable"
                );
                continue;
            }
            match self
                .store
                .acknowledge_runtime_cleanup(&lease, &cleanup)
                .await
            {
                Ok(()) => reclaimed.orphaned_runtimes += 1,
                Err(error) => tracing::warn!(
                    session_id = %cleanup.session_id,
                    runtime = %cleanup.runtime_id,
                    %error,
                    "runtime was removed but cleanup acknowledgement failed; retry remains safe"
                ),
            }
        }
        if let Err(error) = self.store.release_session_lease(&lease).await {
            tracing::debug!(
                session_id = %lease.session_id,
                %error,
                "cleanup lease ended before explicit release"
            );
        }
    }

    /// Read a fresh boundary and acquire its exact reclaim lease.
    async fn acquire_reclaim_lease(
        &self,
        id: &crate::ids::SessionId,
    ) -> Option<(SessionRecord, crate::coordination::SessionLease)> {
        let record = match self.store.get(id).await {
            Ok(Some(record)) => record,
            Ok(None) | Err(StoreError::SessionDeleted { .. }) => return None,
            Err(error) => {
                tracing::warn!(session_id = %id, %error, "cannot refresh session before cleanup");
                return None;
            }
        };
        let authority = match self.store.runtime_authority(id).await {
            Ok(Some(authority)) => authority,
            Ok(None) | Err(StoreError::CoordinationDisabled) => return None,
            Err(error) => {
                tracing::warn!(session_id = %id, %error, "cannot refresh runtime authority before cleanup");
                return None;
            }
        };
        let request = SessionLeaseRequest::new(
            id.clone(),
            self.coordination_holder.clone(),
            SessionBoundary {
                status: record.session.status,
                authority,
            },
            SessionOperation::Reclaim,
            RECLAIM_LEASE_TTL,
        );
        match self.store.acquire_session_lease(&request).await {
            Ok(lease) => Some((record, lease)),
            Err(
                StoreError::SessionLeaseHeld { .. }
                | StoreError::SessionAuthorityChanged { .. }
                | StoreError::SessionLeaseLost { .. }
                | StoreError::SessionDeleted { .. }
                | StoreError::CoordinationDisabled,
            ) => None,
            Err(error) => {
                tracing::warn!(session_id = %id, %error, "cannot acquire runtime cleanup lease");
                None
            }
        }
    }

    /// Give one session whatever it is missing.
    async fn settle_session(
        self: &Arc<Self>,
        mut record: SessionRecord,
        census: &HashMap<KernelId, RuntimeCensus>,
        reclaimed: &mut Reclaimed,
    ) {
        let id = record.session.id.clone();

        // A bring-up already has it. Interfering would give one session two
        // sandboxes.
        if self.starting.read().await.contains(&id) {
            return;
        }

        // The compatibility reconciler below can record loss, rotate, readopt,
        // and provision. Every one of those paths mutates provider or runtime
        // ownership without carrying a durable lease. Once a non-zero authority
        // exists, entering any of them would either bypass that authority or
        // leave the session record disagreeing with it. Coordinated settlement
        // is therefore one indivisible rollout boundary: until its lease-aware
        // implementation is installed, cleanup debt may be reclaimed above but
        // the current authority is otherwise left exactly as it is.
        let coordinated_authority = match self.store.runtime_authority(&id).await {
            Ok(Some(authority)) if authority.generation != 0 => {
                if authority.runtime_id != record.runtime_id {
                    tracing::error!(
                        session_id = %id,
                        record_runtime = ?record.runtime_id,
                        authority_runtime = ?authority.runtime_id,
                        generation = authority.generation,
                        "session record disagrees with coordinated runtime authority"
                    );
                    reclaimed.deferred_sessions += 1;
                    return;
                }
                Some(authority)
            }
            Ok(Some(_)) | Err(StoreError::CoordinationDisabled) => None,
            Ok(None) | Err(StoreError::SessionDeleted { .. }) => return,
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "cannot read runtime authority before reconciliation"
                );
                reclaimed.deferred_sessions += 1;
                return;
            }
        };

        // Three answers, not two. "The provider says it is gone" and "the
        // provider could not be asked" are different facts, and collapsing them
        // would replace every live session behind an adapter that merely cannot
        // enumerate.
        let alive: Option<bool> = match census.get(&record.session.kernel) {
            // `serving`, not `handles`. A sandbox that exists but has stopped
            // is not one a session can be settled onto, and reading presence
            // as liveness is what let a session outlive its own runtime.
            Some(view) if view.trusted => Some(
                record
                    .runtime_id
                    .as_deref()
                    .is_some_and(|r| view.serving.contains(r)),
            ),
            Some(_) => None,
            None => {
                tracing::warn!(session_id = %id, kernel = %record.session.kernel, "no runtime for this session's kernel");
                return;
            }
        };

        // Every path in this branch carries the current durable authority:
        // confirmed loss retires it before replacement, rotation retains it
        // until a usable candidate can switch atomically, and readoption holds
        // it while rebuilding this process's attachment.
        if let Some(authority) = coordinated_authority {
            if record.runtime_id.is_none() {
                self.resume_coordinated_runtime(record, authority, census, reclaimed)
                    .await;
                return;
            }
            if alive == Some(false) {
                if let Some(runtime) = record.runtime_id.clone() {
                    let cause = census
                        .get(&record.session.kernel)
                        .and_then(|view| view.stopped_because.get(&runtime))
                        .copied()
                        .map(RuntimeLossCause::from);
                    self.recover_coordinated_runtime(record, runtime, cause, reclaimed)
                        .await;
                }
                return;
            }
            if alive != Some(false)
                && self.needs_rotation(&record)
                && self.rotate(record.clone()).await
            {
                reclaimed.rotated_sessions += 1;
                return;
            }
            if self.entries.read().await.contains_key(&id) {
                return;
            }
            match self.readopt(&record).await {
                Ok(()) => reclaimed.readopted_sessions += 1,
                Err(error) => {
                    tracing::warn!(
                        session_id = %id,
                        %error,
                        "coordinated runtime readoption failed; authority remains unchanged"
                    );
                    reclaimed.deferred_sessions += 1;
                }
            }
            return;
        }

        /*
         * The moment the loss is established, and the only place it is.
         *
         * Not inside the `entries` check below: that map is what this process
         * is serving, and a runtime may die while another process is serving
         * it, or while none is. A replacement control plane starts with that
         * map empty and must still write the loss.
         *
         * Guarded on the record naming a runtime at all: `is_some_and` on an
         * absent id also yields `Some(false)`, and a session that never had a
         * runtime has not lost one.
         */
        if alive == Some(false) {
            if let Some(runtime) = record.runtime_id.clone() {
                let cause = census
                    .get(&record.session.kernel)
                    .and_then(|view| view.stopped_because.get(&runtime))
                    .copied()
                    .map(RuntimeLossCause::from);
                if let Err(error) = self.record_runtime_lost(&mut record, &runtime, cause).await {
                    tracing::error!(
                        session_id = %id,
                        runtime = %runtime,
                        %error,
                        "cannot record that this session's runtime is gone; deferring replacement"
                    );
                    reclaimed.deferred_sessions += 1;
                    return;
                }
                reclaimed.lost_runtimes += 1;
            }
        }

        // A session that is running, but not on what it should be running. Tried
        // before the checks below because it is the one case where a healthy,
        // attached, perfectly alive session still has to be acted on.
        //
        // A refusal falls through rather than returning. A session that cannot
        // be carried forward still has to be adopted and watched like any
        // other — it is going to keep running on the old image, and leaving it
        // unattended because it could not be moved would be the worse failure.
        if alive != Some(false) && self.needs_rotation(&record) && self.rotate(record.clone()).await
        {
            reclaimed.rotated_sessions += 1;
            return;
        }

        if self.entries.read().await.contains_key(&id) {
            if alive != Some(false) {
                // Either it is running, or nobody can say otherwise. Neither is
                // grounds for touching a session this process is serving.
                return;
            }
            // Serving a session whose sandbox is known to be gone: every
            // keystroke is going nowhere. Drop the attachment so it can be
            // rebuilt honestly.
            tracing::warn!(session_id = %id, "the runtime behind a live session has gone");
            self.entries.write().await.remove(&id);
        }

        // Worth trying whenever the runtime is not known to be gone, because
        // attaching settles the question that the census could not: a sandbox
        // that answers is running, whatever anything else says.
        if record.runtime_id.is_some() && alive != Some(false) {
            match self.readopt(&record).await {
                Ok(()) => {
                    reclaimed.readopted_sessions += 1;
                    return;
                }
                Err(e) if alive.is_none() => {
                    /*
                     * Could not be asked, and could not be reached. Those two
                     * failures have one likely cause — the provider is not
                     * answering — and replacing a sandbox on that evidence
                     * abandons a session that is very probably still running,
                     * with its conversation inside it.
                     *
                     * The module's rule, applied in the direction it was
                     * missing from: a runtime is acted on when the record
                     * positively says so, never merely because this process
                     * could not hear it. Waiting costs one sweep. Replacing
                     * costs somebody's work, and has: a provider that became
                     * ready 1.2 seconds after a boot sweep cost two owners
                     * their conversations, and the sandboxes holding them are
                     * still running, unreferenced.
                     */
                    tracing::warn!(
                        session_id = %id,
                        error = %e,
                        "cannot reach this session's runtime and cannot confirm it is gone; leaving it for the next sweep"
                    );
                    reclaimed.deferred_sessions += 1;
                    return;
                }
                Err(crate::error::CoreError::RuntimeRefused(why)) => {
                    /*
                     * The runtime answered, and would not be taken over.
                     *
                     * Never replaced on this. An answer is proof something is
                     * there to have sent it, and replacing it destroys a
                     * working agent along with everything it knows — which is
                     * what happened on every rollout while a refusal and a
                     * silence were the same answer here. A bridge greeted
                     * twice refused the second greeting, and a session was
                     * rebuilt empty beside the pod still holding its
                     * conversation.
                     *
                     * Deferred instead, so the next sweep tries again. A
                     * refusal that clears costs one sweep; one that does not
                     * leaves the session listed and its work intact for its
                     * owner to act on, which is the recoverable failure of the
                     * two available here.
                     */
                    tracing::warn!(
                        session_id = %id,
                        %why,
                        "this runtime answered and refused the takeover; leaving it alone"
                    );
                    reclaimed.deferred_sessions += 1;
                    return;
                }
                Err(e) => {
                    // The provider was asked and says this runtime is there.
                    // Listed but unreachable is no more usable than gone.
                    tracing::warn!(session_id = %id, error = %e, "cannot take over the runtime");
                }
            }

            // Readoption performs network work and can lose a race with a stop,
            // termination, deletion, or replacement runtime. Never reprovision
            // from the old sweep snapshot after that: reread the desired state
            // and defer if it now names a different runtime.
            let latest = match self.store.get(&id).await {
                Ok(Some(latest)) if latest.session.status.wants_runtime() => latest,
                Ok(_) => return,
                Err(error) => {
                    tracing::warn!(session_id = %id, %error, "cannot recheck the session after readoption failed");
                    reclaimed.deferred_sessions += 1;
                    return;
                }
            };
            if latest.runtime_id != record.runtime_id
                || latest.session.kernel != record.session.kernel
            {
                tracing::debug!(session_id = %id, "the session changed while readoption was in flight; leaving its new state for the next pass");
                reclaimed.deferred_sessions += 1;
                return;
            }
            record = latest;
        }

        self.reprovision(record, reclaimed).await;
    }

    /// Retire a runtime positively confirmed lost and start its replacement.
    ///
    /// The store clears authority and creates exact cleanup debt atomically.
    /// Input remains write-fenced until the loss is durable and the stale live
    /// entry is gone, so no operation accepted for the former runtime can cross
    /// the retirement boundary.
    async fn recover_coordinated_runtime(
        self: &Arc<Self>,
        record: SessionRecord,
        runtime: String,
        cause: Option<RuntimeLossCause>,
        reclaimed: &mut Reclaimed,
    ) {
        let id = record.session.id.clone();
        let Some(adapter) = self.adapters.get(&record.session.kernel).cloned() else {
            tracing::warn!(
                session_id = %id,
                kernel = %record.session.kernel,
                "cannot recover a lost runtime without its adapter"
            );
            reclaimed.deferred_sessions += 1;
            return;
        };
        if !self.begin_start(&id).await {
            return;
        }

        let mut lease = match self
            .acquire_runtime_lease_at(&record, SessionOperation::Recover)
            .await
        {
            Ok(Some(lease)) => lease,
            Ok(None) => {
                tracing::error!(
                    session_id = %id,
                    "coordinated loss recovery unexpectedly received no durable lease"
                );
                self.finish_start(&id).await;
                reclaimed.deferred_sessions += 1;
                return;
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "cannot acquire authority to recover the lost runtime"
                );
                self.finish_start(&id).await;
                reclaimed.deferred_sessions += 1;
                return;
            }
        };

        let input_fence = self.input_gates.gate(&id).write_owned().await;
        let retired = match self.retire_current_runtime(&mut lease).await {
            Ok(retired) => retired,
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    runtime = %runtime,
                    %error,
                    "cannot retire the lost runtime authority"
                );
                drop(input_fence);
                self.finish_failed_coordinated_recovery(&id, &lease, false, reclaimed)
                    .await;
                return;
            }
        };
        if retired.cleanup.runtime_id != runtime {
            tracing::error!(
                session_id = %id,
                observed_runtime = %runtime,
                retired_runtime = %retired.cleanup.runtime_id,
                "retired authority does not match the runtime confirmed lost"
            );
            drop(input_fence);
            self.finish_failed_coordinated_recovery(&id, &lease, true, reclaimed)
                .await;
            return;
        }

        let mut recovering = match self.store.get(&id).await {
            Ok(Some(current))
                if current.runtime_id.is_none()
                    && current.session.status == lease.boundary.status
                    && current.session.kernel == record.session.kernel =>
            {
                current
            }
            Ok(Some(_)) => {
                tracing::error!(
                    session_id = %id,
                    "session changed after its lost runtime authority was retired"
                );
                drop(input_fence);
                self.finish_failed_coordinated_recovery(&id, &lease, true, reclaimed)
                    .await;
                return;
            }
            Ok(None) | Err(StoreError::SessionDeleted { .. }) => {
                tracing::warn!(session_id = %id, "session disappeared during runtime recovery");
                drop(input_fence);
                self.finish_failed_coordinated_recovery(&id, &lease, true, reclaimed)
                    .await;
                return;
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "cannot reload the session after retiring its lost runtime"
                );
                drop(input_fence);
                self.finish_failed_coordinated_recovery(&id, &lease, true, reclaimed)
                    .await;
                return;
            }
        };
        if let Err(error) = self
            .record_runtime_lost(&mut recovering, &runtime, cause)
            .await
        {
            tracing::error!(
                session_id = %id,
                runtime = %runtime,
                %error,
                "lost authority was retired but its loss could not be recorded"
            );
            drop(input_fence);
            self.finish_failed_coordinated_recovery(&id, &lease, true, reclaimed)
                .await;
            return;
        }
        reclaimed.lost_runtimes += 1;
        self.entries.write().await.remove(&id);
        drop(input_fence);

        self.launch_coordinated_replacement(
            recovering,
            adapter,
            lease,
            Some(retired.cleanup),
            reclaimed,
        )
        .await;
    }

    /// Continue recovery after a process died with authority already absent.
    ///
    /// `Provisioning` or `Recovering` proves the loss/revive transition was
    /// already recorded. Otherwise the current-generation retired cleanup debt
    /// supplies the exact former runtime id needed to finish the loss event.
    async fn resume_coordinated_runtime(
        self: &Arc<Self>,
        record: SessionRecord,
        authority: RuntimeAuthority,
        census: &HashMap<KernelId, RuntimeCensus>,
        reclaimed: &mut Reclaimed,
    ) {
        let id = record.session.id.clone();
        if record.session.status == SessionStatus::Rotating {
            tracing::warn!(
                session_id = %id,
                generation = authority.generation,
                "cannot resume a handover whose former authority is already absent"
            );
            reclaimed.deferred_sessions += 1;
            return;
        }
        let Some(adapter) = self.adapters.get(&record.session.kernel).cloned() else {
            tracing::warn!(
                session_id = %id,
                kernel = %record.session.kernel,
                "cannot resume runtime recovery without its adapter"
            );
            reclaimed.deferred_sessions += 1;
            return;
        };
        if !self.begin_start(&id).await {
            return;
        }

        let needs_loss = record.session.status != SessionStatus::Provisioning
            && record.session.recovery_state != RecoveryState::Recovering;
        let lost_cleanup = if needs_loss {
            let debts = match self.store.runtime_cleanup_debts(&id).await {
                Ok(debts) => debts,
                Err(error) => {
                    tracing::warn!(
                        session_id = %id,
                        %error,
                        "cannot recover the retired runtime identity"
                    );
                    self.finish_start(&id).await;
                    reclaimed.deferred_sessions += 1;
                    return;
                }
            };
            let mut matching = debts.into_iter().filter(|cleanup| {
                cleanup.reason == RuntimeCleanupReason::RetiredAuthority
                    && cleanup.generation == authority.generation
            });
            let cleanup = matching.next();
            if cleanup.is_none() || matching.next().is_some() {
                tracing::error!(
                    session_id = %id,
                    generation = authority.generation,
                    "no unique retired runtime identity remains for the unrecorded loss"
                );
                self.finish_start(&id).await;
                reclaimed.deferred_sessions += 1;
                return;
            }
            cleanup
        } else {
            None
        };

        let lease = match self
            .acquire_runtime_lease_at(&record, SessionOperation::Recover)
            .await
        {
            Ok(Some(lease)) => lease,
            Ok(None) => {
                tracing::error!(
                    session_id = %id,
                    "coordinated recovery resume unexpectedly received no durable lease"
                );
                self.finish_start(&id).await;
                reclaimed.deferred_sessions += 1;
                return;
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "cannot reacquire the interrupted recovery boundary"
                );
                self.finish_start(&id).await;
                reclaimed.deferred_sessions += 1;
                return;
            }
        };

        let input_fence = self.input_gates.gate(&id).write_owned().await;
        if let Err(error) = self.renew_runtime_lease(Some(&lease)).await {
            tracing::warn!(session_id = %id, %error, "interrupted recovery lease could not be renewed");
            drop(input_fence);
            self.finish_failed_coordinated_recovery(&id, &lease, false, reclaimed)
                .await;
            return;
        }
        let mut current = match self.store.get(&id).await {
            Ok(Some(current))
                if current.runtime_id.is_none()
                    && SessionBoundary::for_record(&current, authority.generation)
                        == lease.boundary =>
            {
                current
            }
            Ok(Some(_)) => {
                tracing::debug!(session_id = %id, "interrupted recovery boundary changed");
                drop(input_fence);
                self.finish_failed_coordinated_recovery(&id, &lease, false, reclaimed)
                    .await;
                return;
            }
            Ok(None) | Err(StoreError::SessionDeleted { .. }) => {
                tracing::warn!(session_id = %id, "session disappeared during recovery resume");
                drop(input_fence);
                self.finish_failed_coordinated_recovery(&id, &lease, true, reclaimed)
                    .await;
                return;
            }
            Err(error) => {
                tracing::warn!(session_id = %id, %error, "cannot reload interrupted recovery");
                drop(input_fence);
                self.finish_failed_coordinated_recovery(&id, &lease, false, reclaimed)
                    .await;
                return;
            }
        };

        if let Some(cleanup) = lost_cleanup.as_ref() {
            let cause = census
                .get(&current.session.kernel)
                .and_then(|view| view.stopped_because.get(&cleanup.runtime_id))
                .copied()
                .map(RuntimeLossCause::from);
            if let Err(error) = self
                .record_runtime_lost(&mut current, &cleanup.runtime_id, cause)
                .await
            {
                tracing::error!(
                    session_id = %id,
                    runtime = %cleanup.runtime_id,
                    %error,
                    "cannot finish recording the interrupted runtime loss"
                );
                drop(input_fence);
                self.finish_failed_coordinated_recovery(&id, &lease, true, reclaimed)
                    .await;
                return;
            }
            reclaimed.lost_runtimes += 1;
        }
        self.entries.write().await.remove(&id);
        drop(input_fence);

        self.launch_coordinated_replacement(current, adapter, lease, lost_cleanup, reclaimed)
            .await;
    }

    /// Move a held no-runtime recovery boundary into provisioning and detach it.
    async fn launch_coordinated_replacement(
        self: &Arc<Self>,
        mut starting: SessionRecord,
        adapter: Arc<dyn layer3_adapter::KernelAdapter>,
        mut lease: SessionLease,
        cleanup: Option<RuntimeCleanup>,
        reclaimed: &mut Reclaimed,
    ) {
        let id = starting.session.id.clone();
        if starting.runtime_id.is_some() {
            tracing::error!(session_id = %id, "replacement launch still names a runtime");
            self.finish_failed_coordinated_recovery(&id, &lease, true, reclaimed)
                .await;
            return;
        }
        if starting.session.status != SessionStatus::Provisioning
            || starting.session.stop_reason.is_some()
        {
            starting.session.status = SessionStatus::Provisioning;
            starting.session.stop_reason = None;
            if let Err(error) = self.persist_durably(&mut starting, Vec::new()).await {
                tracing::error!(
                    session_id = %id,
                    %error,
                    "cannot record coordinated replacement provisioning"
                );
                self.finish_failed_coordinated_recovery(&id, &lease, true, reclaimed)
                    .await;
                return;
            }
        }

        let boundary = SessionBoundary::for_record(&starting, lease.boundary.authority.generation);
        if boundary != lease.boundary {
            if let Err(error) = self
                .store
                .advance_session_lease(&mut lease, &boundary)
                .await
            {
                tracing::error!(
                    session_id = %id,
                    %error,
                    "cannot advance the coordinated recovery lease to provisioning"
                );
                self.finish_failed_coordinated_recovery(&id, &lease, true, reclaimed)
                    .await;
                return;
            }
        }

        if let Some(cleanup) = cleanup.as_ref() {
            if let Err(error) = self
                .reclaim_runtime_with_lease(&adapter, &lease, cleanup)
                .await
            {
                tracing::warn!(
                    session_id = %id,
                    runtime = %cleanup.runtime_id,
                    %error,
                    "lost runtime cleanup remains durable while replacement continues"
                );
            }
        }

        let geometry = starting.session.geometry;
        let spec = SessionSpec {
            geometry: Some(geometry),
            options: starting.session.options.clone(),
            environment_profile: starting.session.environment_profile.clone(),
            caller_credential: None,
            runtime_credential: None,
            requested_mode: None,
            quoted_rate_card: None,
        };
        tracing::warn!(session_id = %id, "starting a coordinated replacement runtime");
        let service = Arc::clone(self);
        let session = starting.session;
        tokio::spawn(async move {
            service
                .bring_up_with_lease(
                    session,
                    adapter,
                    spec,
                    geometry,
                    None,
                    crate::provision_failure::Abandonment::Keep,
                    Some(lease),
                    None,
                )
                .await;
        });
        reclaimed.reprovisioned_sessions += 1;
    }

    /// Release every local and durable claim after recovery cannot continue.
    async fn finish_failed_coordinated_recovery(
        &self,
        id: &crate::ids::SessionId,
        lease: &SessionLease,
        discard_entry: bool,
        reclaimed: &mut Reclaimed,
    ) {
        if discard_entry {
            self.entries.write().await.remove(id);
        }
        if let Err(error) = self.store.release_session_lease(lease).await {
            tracing::debug!(
                session_id = %id,
                %error,
                "recovery lease ended before explicit release"
            );
        }
        self.finish_start(id).await;
        reclaimed.deferred_sessions += 1;
    }

    /// Record that a session's runtime is gone, and stop naming it.
    ///
    /// One record write holds both facts. A record that still names a runtime
    /// this sweep declared dead would have every later sweep declare it dead
    /// again, filling the log with the same loss every five minutes.
    ///
    /// The session moves to `Recovering`, which is what that state means. It is
    /// not a failure state; the same pass starts a replacement runtime.
    async fn record_runtime_lost(
        &self,
        record: &mut SessionRecord,
        runtime: &str,
        cause: Option<RuntimeLossCause>,
    ) -> Result<(), StoreError> {
        let id = record.session.id.clone();
        let previous = record.runtime_id.take();
        let previous_state = record.session.recovery_state;
        record.session.recovery_state = RecoveryState::Recovering;
        if let Err(e) = self
            .persist_durably(
                record,
                vec![Event {
                    session_id: id.clone(),
                    seq: 0,
                    kind: EventKind::RuntimeLost {
                        runtime_id: runtime.to_string(),
                        cause,
                    },
                    appended_at: String::new(),
                }],
            )
            .await
        {
            // Nothing was recorded, so nothing is claimed. Put the local copy
            // back and let a later sweep establish the loss again.
            record.runtime_id = previous;
            record.session.recovery_state = previous_state;
            tracing::error!(session_id = %id, runtime = %runtime, error = %e,
                "cannot record that this session's runtime is gone");
            return Err(e);
        }
        tracing::warn!(session_id = %id, runtime = %runtime, ?cause,
            "this session's runtime is gone");
        Ok(())
    }

    /// Start a new runtime for a session that should have one.
    ///
    /// The session is not recreated — same id, same history, same options — so
    /// everything already referring to it keeps working. Only the sandbox is
    /// new, and the sandbox was never the session.
    async fn reprovision(self: &Arc<Self>, record: SessionRecord, reclaimed: &mut Reclaimed) {
        let id = record.session.id.clone();
        let was_rotating = record.session.status.is_rotating();
        let Some(adapter) = self.adapters.get(&record.session.kernel).cloned() else {
            return;
        };
        if !self.begin_start(&id).await {
            return;
        }
        if was_rotating {
            // A replacement process has no in-memory rotation marker to inherit.
            // Preserve the operation's identity before rewriting the durable
            // status to `Provisioning`, through readiness and live installation.
            self.rotations_in_progress.write().await.insert(id.clone());
        }

        let mut starting = record.clone();
        // A runtime it no longer has must not still be named, or the next sweep
        // reads a stale id and believes the session is fine.
        starting.runtime_id = None;
        if starting.session.status != SessionStatus::Provisioning {
            starting.session.status = SessionStatus::Provisioning;
        }
        starting.session.stop_reason = None;
        if let Err(e) = self.persist_durably(&mut starting, Vec::new()).await {
            tracing::error!(session_id = %id, error = %e, "cannot record the restart");
            self.finish_start(&id).await;
            return;
        }

        tracing::warn!(session_id = %id, "starting a new runtime for a session that lost one");
        let geometry = starting.session.geometry;
        let spec = SessionSpec {
            geometry: Some(geometry),
            options: starting.session.options.clone(),
            environment_profile: starting.session.environment_profile.clone(),
            // Nobody is here to delegate one; the renewal loop supplies it.
            caller_credential: None,
            runtime_credential: None,
            requested_mode: None,
            quoted_rate_card: None,
        };
        let service = Arc::clone(self);
        let session = starting.session;
        tokio::spawn(async move {
            service
                .bring_up(
                    session,
                    spec,
                    adapter,
                    geometry,
                    None,
                    crate::provision_failure::Abandonment::Keep,
                )
                .await;
        });
        reclaimed.reprovisioned_sessions += 1;
    }
}
