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
use crate::store::{PageRequest, SessionRecord, SessionScan, SkillPackState, StoreError};

const RECLAIM_LEASE_TTL: Duration = Duration::from_secs(30);
const CHAT_DISPATCH_FENCE_TIMEOUT_SECONDS: u64 = 10 * 60;

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
    /// Runtime id to its shared chat-host metadata. These objects are excluded
    /// from ordinary one-session ownership accounting.
    chat_hosts: HashMap<String, String>,
    trusted: bool,
}

fn same_compatibility_runtime_boundary(observed: &SessionRecord, current: &SessionRecord) -> bool {
    observed.session.id == current.session.id
        && observed.session.kernel == current.session.kernel
        && observed.session.status == current.session.status
        && observed.rotation_claim == current.rotation_claim
        && observed.runtime_id == current.runtime_id
}

impl SessionService {
    /// Compare the provider's runtimes against the record, and settle both.
    pub async fn reconcile(self: &Arc<Self>) -> Reclaimed {
        let mut reclaimed = Reclaimed::default();

        // Preserve the original fail-closed boundary: prove that the complete
        // durable set is readable before asking providers or taking any action.
        // Pages are dropped as they are validated, so that proof no longer
        // retains every historical session in one process-wide allocation.
        // This cannot be folded into the action walk: then a later page failure
        // would arrive only after earlier cleanup mutations had already landed.
        let mut preflight = SessionScan::new();
        loop {
            match preflight.next_page(self.store.as_ref()).await {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => {
                    // Without the record there is nothing to compare against,
                    // and guessing here would tear down live work.
                    tracing::error!(
                        error = %error,
                        "reconciliation skipped: cannot read sessions"
                    );
                    return reclaimed;
                }
            }
        }

        let census = self.take_census().await;
        self.reconcile_chat_hosts(&census).await;

        // Provider ownership is the bounded set relevant to orphan diagnostics.
        // Remove an id only after its entitled durable row is observed; a
        // partial scan must never be treated as evidence that the row is absent.
        let mut unaccounted_sessions: HashSet<String> = census
            .values()
            .filter(|view| view.trusted)
            .flat_map(|view| view.owners.values().filter_map(Clone::clone))
            .collect();

        let mut action_scan = SessionScan::new();
        loop {
            let records = match action_scan.next_page(self.store.as_ref()).await {
                Ok(Some(records)) => records,
                Ok(None) => break,
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        "reconciliation stopped: cannot continue cleanup session scan"
                    );
                    return reclaimed;
                }
            };
            for record in &records {
                if record.chat_origin.is_some() {
                    continue;
                }
                self.reconcile_terminal_skill_pack_cleanup(record).await;
                self.discard_stale_coordinated_entry(record).await;
                self.reclaim_cleanup_debts(record, &mut reclaimed).await;
                if !record.session.wants_runtime() && record.runtime_id.is_some() {
                    self.settle_stopped_compatibility_runtime(record, &mut reclaimed)
                        .await;
                }
                if record.session.wants_runtime() {
                    unaccounted_sessions.remove(record.session.id.as_str());
                }
            }
        }

        for view in census.values() {
            if !view.trusted {
                continue;
            }
            for (runtime, session) in &view.owners {
                if session
                    .as_deref()
                    .is_some_and(|id| !unaccounted_sessions.contains(id))
                {
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

        // Keep settlement in its own walk so every cleanup phase completes
        // globally before reconciliation publishes or replaces a live runtime.
        let mut settlement_scan = SessionScan::new();
        loop {
            let records = match settlement_scan.next_page(self.store.as_ref()).await {
                Ok(Some(records)) => records,
                Ok(None) => break,
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        "reconciliation stopped: cannot continue settlement session scan"
                    );
                    return reclaimed;
                }
            };
            for record in records {
                if record.chat_origin.is_some() {
                    if self.reconcile_chat_thread(&record).await {
                        reclaimed.readopted_sessions += 1;
                    }
                    continue;
                }
                if !record.session.wants_runtime() {
                    continue;
                }
                self.settle_session(record, &census, &mut reclaimed).await;
            }
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

    /// Reconnect a thread only to the exact active shared host. Host recovery,
    /// pause, rotation, and loss remain exclusively owned by the host aggregate.
    async fn reconcile_chat_thread(self: &Arc<Self>, record: &SessionRecord) -> bool {
        if !record.session.wants_runtime()
            || self.entries.read().await.contains_key(&record.session.id)
        {
            return false;
        }
        let Some(origin) = record.chat_origin.as_ref() else {
            return false;
        };
        let Some(store) = self.chat_hosts.as_ref() else {
            tracing::warn!(
                session_id = %record.session.id,
                "chat thread reconciliation deferred because host storage is unavailable"
            );
            return false;
        };
        let host = match store.get(&origin.host_id).await {
            Ok(Some(host)) => host,
            Ok(None) => {
                tracing::warn!(
                    session_id = %record.session.id,
                    chat_host = %origin.host_id,
                    "chat thread reconciliation deferred because its host record is absent"
                );
                return false;
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %record.session.id,
                    %error,
                    "chat thread reconciliation deferred because its host could not be read"
                );
                return false;
            }
        };
        if host.status != crate::ChatHostStatus::Active
            || host.kernel != record.session.kernel
            || host.runtime_id != record.runtime_id
            || !host
                .threads
                .iter()
                .any(|slot| slot.session_id == record.session.id && !slot.removed)
        {
            return false;
        }
        match self.readopt(record).await {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    session_id = %record.session.id,
                    chat_host = %origin.host_id,
                    %error,
                    "active chat thread could not be reconnected"
                );
                false
            }
        }
    }

    /// Replay terminal object cleanup from the durable owner row.
    ///
    /// A scan is only a reason to investigate. The shared cleanup driver
    /// refreshes the row, acquires a distributed Reclaim lease before the local
    /// lifecycle gate, and releases that lease on every result.
    async fn reconcile_terminal_skill_pack_cleanup(&self, observed: &SessionRecord) {
        if !observed.session.status.is_terminal() || observed.skill_packs == SkillPackState::Cleaned
        {
            return;
        }
        let id = &observed.session.id;
        match self
            .finish_terminal_skill_pack_cleanup_with_authority(id, SessionOperation::Reclaim)
            .await
        {
            Ok(_) | Err(StoreError::SessionDeleted { .. }) => {}
            Err(StoreError::SessionLeaseHeld { .. }) => tracing::debug!(
                session_id = %id,
                "terminal skill cleanup is owned by another lifecycle operation"
            ),
            Err(error) => tracing::warn!(
                session_id = %id,
                %error,
                "terminal session skill cleanup remains pending"
            ),
        }
    }

    /// Remove a replica-local channel that no longer names durable authority.
    ///
    /// This pass includes stopped sessions, which ordinary settlement skips.
    /// Without it, a stop on one replica can leave another replica's attachment
    /// and output relay presenting the retired runtime indefinitely even though
    /// exact interaction checks already refuse new input.
    async fn discard_stale_coordinated_entry(&self, record: &SessionRecord) {
        let id = &record.session.id;
        let authority = match self.store.runtime_authority(id).await {
            Ok(Some(authority)) if authority.generation != 0 => authority,
            Ok(Some(_)) | Ok(None) | Err(StoreError::CoordinationDisabled) => return,
            Err(error) => {
                tracing::debug!(
                    session_id = %id,
                    %error,
                    "cannot compare the local runtime entry with durable authority"
                );
                return;
            }
        };
        let suspected_stale = self.entries.read().await.get(id).is_some_and(|entry| {
            entry.session.status.is_rotating()
                || !entry.publication_complete
                || entry.runtime_generation != authority.generation
                || entry.runtime.as_ref().map(|runtime| runtime.0.as_str())
                    != authority.runtime_id.as_deref()
        });
        if !suspected_stale {
            return;
        }

        // The first comparison is only a cheap reason to investigate. Acquire a
        // fresh durable boundary before changing local state so a concurrent
        // readoption or switch cannot publish a successor between comparison and
        // removal. Durable lease first, then the local lifecycle/input gates,
        // preserves the provider-operation lock order.
        let mut current = match self.store.get(id).await {
            Ok(Some(current)) => current,
            _ => return,
        };
        let mut resumed_local_cleanup = false;
        let lease = loop {
            match self
                .acquire_runtime_lease_at(&current, SessionOperation::Reclaim)
                .await
            {
                Ok(Some(lease)) => break lease,
                Ok(None) => return,
                Err(StoreError::SessionLeaseHeld { .. }) if !resumed_local_cleanup => {
                    resumed_local_cleanup = true;
                    self.event_logs.drain_retained_cleanups(id).await;
                    current = match self.store.get(id).await {
                        Ok(Some(current)) => current,
                        _ => return,
                    };
                }
                Err(_) => return,
            }
        };
        let lifecycle = self.lock_lifecycle(id).await;
        let input_fence = self.input_gates.gate(id).write_owned().await;
        if let Err(error) = self.renew_runtime_lease(Some(&lease)).await {
            tracing::debug!(session_id = %id, %error, "stale-entry lease expired before local removal");
            drop(input_fence);
            drop(lifecycle);
            let _ = self.store.release_session_lease(&lease).await;
            return;
        }
        let authority = &lease.boundary.authority;
        let removed = {
            let mut entries = self.entries.write().await;
            let stale = entries.get(id).is_some_and(|entry| {
                entry.session.status.is_rotating()
                    || !entry.publication_complete
                    || entry.runtime_generation != authority.generation
                    || entry.runtime.as_ref().map(|runtime| runtime.0.as_str())
                        != authority.runtime_id.as_deref()
            });
            stale.then(|| entries.remove(id)).flatten()
        };
        drop(input_fence);
        drop(lifecycle);
        if let Some(entry) = removed {
            if let Some(attached) = entry.attached.as_ref() {
                let _ = attached
                    .supersede
                    .send(crate::lease::Detached::RuntimeReplaced);
            }
            tracing::warn!(
                session_id = %id,
                local_runtime = ?entry.runtime.as_ref().map(|runtime| runtime.0.as_str()),
                local_generation = entry.runtime_generation,
                durable_runtime = ?authority.runtime_id,
                durable_generation = authority.generation,
                "discarded a replica-local entry outside durable runtime authority"
            );
        }
        if let Err(error) = self.store.release_session_lease(&lease).await {
            tracing::debug!(session_id = %id, %error, "stale-entry lease ended before explicit release");
        }
    }

    /// Remove a stale local channel for a store that has no durable leases.
    ///
    /// The local lifecycle and input gates exclude this process's handover and
    /// deliveries; a fresh exact revision then prevents an old sweep from
    /// removing an entry after durable state changed while it waited. A local
    /// H1 distinct from durable H2 is terminated while its entry still retains
    /// the only cleanup identity. Provider failure leaves that entry intact for
    /// the next sweep.
    async fn discard_stale_compatibility_entry(&self, record: &SessionRecord) -> bool {
        let id = &record.session.id;
        let lifecycle = self.lock_lifecycle(id).await;
        let input_fence = self.input_gates.gate(id).write_owned().await;
        match self.store.runtime_authority(id).await {
            Ok(Some(authority)) if authority.generation == 0 => {}
            Err(StoreError::CoordinationDisabled) => {}
            _ => return false,
        }
        let current = match self.store.get(id).await {
            Ok(Some(current)) if current.revision == record.revision => current,
            _ => return false,
        };
        let stale = {
            let entries = self.entries.read().await;
            entries.get(id).and_then(|entry| {
                (entry.session.status.is_rotating()
                    || !entry.publication_complete
                    || entry.runtime_generation != 0
                    || entry.runtime.as_ref().map(|runtime| runtime.0.as_str())
                        != current.runtime_id.as_deref())
                .then(|| {
                    (
                        entry.runtime.clone(),
                        entry.session.kernel.clone(),
                        Arc::clone(&entry.screen),
                    )
                })
            })
        };
        let Some((local_runtime, local_kernel, entry_screen)) = stale else {
            return false;
        };
        if local_runtime.as_ref().map(|runtime| runtime.0.as_str()) != current.runtime_id.as_deref()
        {
            let Some(adapter) = self.adapters.get(&local_kernel) else {
                tracing::warn!(
                    session_id = %id,
                    kernel = %local_kernel,
                    local_runtime = ?local_runtime.as_ref().map(|runtime| runtime.0.as_str()),
                    "cannot discard a stale compatibility runtime without its adapter"
                );
                return false;
            };
            if let Err(error) = self
                .require_compatibility_provider_mutation(id, "stale runtime reconciliation")
                .await
            {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "cannot authorize stale compatibility runtime teardown"
                );
                return false;
            }
            if let Some(handle) = local_runtime.as_ref() {
                if let Err(error) = adapter.terminate(handle).await {
                    tracing::warn!(
                        session_id = %id,
                        runtime = %handle.0,
                        %error,
                        "stale compatibility runtime teardown failed; retaining its local entry"
                    );
                    return false;
                }
            }
        }
        let removed = {
            let mut entries = self.entries.write().await;
            let exact = entries
                .get(id)
                .is_some_and(|entry| Arc::ptr_eq(&entry.screen, &entry_screen));
            exact.then(|| entries.remove(id)).flatten()
        };
        drop(input_fence);
        drop(lifecycle);
        if let Some(entry) = removed {
            if let Some(attached) = entry.attached.as_ref() {
                let _ = attached
                    .supersede
                    .send(crate::lease::Detached::RuntimeReplaced);
            }
            tracing::warn!(
                session_id = %id,
                local_runtime = ?entry.runtime.as_ref().map(|runtime| runtime.0.as_str()),
                durable_runtime = ?current.runtime_id,
                "settled and discarded a stale compatibility runtime entry"
            );
            true
        } else {
            false
        }
    }

    /// Finish provider cleanup left behind by a failed compatibility stop.
    ///
    /// The scanned record is only a reason to investigate. Provider mutation
    /// uses a fresh lifecycle boundary after local input is fenced, and the
    /// runtime remains durable until teardown succeeds so another sweep can
    /// retry it.
    async fn settle_stopped_compatibility_runtime(
        &self,
        observed: &SessionRecord,
        reclaimed: &mut Reclaimed,
    ) {
        let id = &observed.session.id;
        let _lifecycle = self.lock_lifecycle(id).await;
        let _input_fence = self.input_gates.gate(id).write_owned().await;
        let current = match self.store.get(id).await {
            Ok(Some(current)) if matches!(&current.skill_packs, SkillPackState::StagedSummary) => {
                tracing::error!(
                    session_id = %id,
                    "exact stopped-session read returned a projected skill custody summary"
                );
                reclaimed.deferred_sessions += 1;
                return;
            }
            Ok(Some(current)) => current,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "cannot refresh a stopped compatibility runtime before cleanup"
                );
                reclaimed.deferred_sessions += 1;
                return;
            }
        };
        if current.session.wants_runtime() || current.runtime_id.is_none() {
            return;
        }
        if !same_compatibility_runtime_boundary(observed, &current) {
            tracing::debug!(
                session_id = %id,
                "the stopped compatibility runtime changed before cleanup"
            );
            reclaimed.deferred_sessions += 1;
            return;
        }
        let local = {
            let entries = self.entries.read().await;
            entries.get(id).map(|entry| {
                (
                    entry.runtime.clone(),
                    entry.session.kernel.clone(),
                    Arc::clone(&entry.screen),
                )
            })
        };
        if let Some((Some(local_runtime), local_kernel, _)) = local.as_ref() {
            if current.runtime_id.as_deref() != Some(local_runtime.0.as_str()) {
                let Some(local_adapter) = self.adapters.get(local_kernel) else {
                    tracing::warn!(
                        session_id = %id,
                        kernel = %local_kernel,
                        runtime = %local_runtime.0,
                        "cannot clean up a stopped local compatibility runtime without its adapter"
                    );
                    reclaimed.deferred_sessions += 1;
                    return;
                };
                if let Err(error) = self
                    .require_compatibility_provider_mutation(
                        id,
                        "stopped local runtime reconciliation",
                    )
                    .await
                {
                    tracing::warn!(
                        session_id = %id,
                        runtime = %local_runtime.0,
                        %error,
                        "cannot authorize stopped local compatibility runtime teardown"
                    );
                    reclaimed.deferred_sessions += 1;
                    return;
                }
                if let Err(error) = local_adapter.terminate(local_runtime).await {
                    tracing::warn!(
                        session_id = %id,
                        runtime = %local_runtime.0,
                        %error,
                        "stopped local compatibility runtime teardown failed; retaining its entry"
                    );
                    reclaimed.deferred_sessions += 1;
                    return;
                }
            }
        }
        let Some(adapter) = self.adapters.get(&current.session.kernel).cloned() else {
            tracing::warn!(
                session_id = %id,
                kernel = %current.session.kernel,
                "cannot clean up a stopped compatibility runtime without its adapter"
            );
            reclaimed.deferred_sessions += 1;
            return;
        };
        let runtime = current
            .runtime_id
            .clone()
            .expect("a stopped cleanup candidate retains its runtime");
        if let Err(error) = self
            .teardown_compatibility_runtime(&adapter, &current, "stopped runtime reconciliation")
            .await
        {
            tracing::warn!(
                session_id = %id,
                runtime = %runtime,
                %error,
                "stopped compatibility runtime cleanup remains pending"
            );
            reclaimed.deferred_sessions += 1;
            return;
        }

        if current.session.status.is_terminal() {
            if let Some(source) = self.credentials.as_ref() {
                if let Err(error) = source.release(id).await {
                    tracing::error!(
                        session_id = %id,
                        %error,
                        "could not retire terminal session credential custody after runtime cleanup"
                    );
                }
            }
        }

        let removed = {
            let mut entries = self.entries.write().await;
            let exact = local.as_ref().is_some_and(|(_, _, screen)| {
                entries
                    .get(id)
                    .is_some_and(|entry| Arc::ptr_eq(&entry.screen, screen))
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
        tracing::info!(
            session_id = %id,
            runtime = %runtime,
            "finished stopped compatibility runtime cleanup"
        );
    }

    /// Retire the exact compatibility runtime whose readoption just failed.
    async fn clear_failed_compatibility_readoption(
        &self,
        observed: &SessionRecord,
        reclaimed: &mut Reclaimed,
    ) -> Option<SessionRecord> {
        let id = &observed.session.id;
        let _lifecycle = self.lock_lifecycle(id).await;
        let _input_fence = self.input_gates.gate(id).write_owned().await;
        let current = match self.store.get(id).await {
            Ok(Some(current)) => current,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "cannot refresh the session after readoption failed"
                );
                reclaimed.deferred_sessions += 1;
                return None;
            }
        };
        if !current.session.wants_runtime()
            || current.runtime_id.is_none()
            || !same_compatibility_runtime_boundary(observed, &current)
        {
            tracing::debug!(
                session_id = %id,
                "the session changed before its failed readoption could be cleaned up"
            );
            reclaimed.deferred_sessions += 1;
            return None;
        }
        if self.entries.read().await.contains_key(id) {
            tracing::debug!(
                session_id = %id,
                "a local runtime entry appeared after readoption failed"
            );
            reclaimed.deferred_sessions += 1;
            return None;
        }
        let Some(adapter) = self.adapters.get(&current.session.kernel).cloned() else {
            tracing::warn!(
                session_id = %id,
                kernel = %current.session.kernel,
                "cannot clean up a failed readoption without its adapter"
            );
            reclaimed.deferred_sessions += 1;
            return None;
        };
        let runtime = current
            .runtime_id
            .clone()
            .expect("a failed readoption cleanup candidate retains its runtime");
        match self
            .teardown_compatibility_runtime(&adapter, &current, "failed runtime readoption")
            .await
        {
            Ok(cleared) => {
                tracing::warn!(
                    session_id = %id,
                    runtime = %runtime,
                    "retired an unreachable compatibility runtime before replacement"
                );
                Some(cleared)
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    runtime = %runtime,
                    %error,
                    "failed readoption runtime cleanup remains pending"
                );
                reclaimed.deferred_sessions += 1;
                None
            }
        }
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
                    let mut chat_hosts = HashMap::new();
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
                        if let Some(chat_host) = listing.chat_host {
                            chat_hosts.insert(listing.handle.0, chat_host);
                            continue;
                        }
                        owners.insert(listing.handle.0, listing.session);
                    }
                    RuntimeCensus {
                        serving,
                        generations,
                        stopped_because,
                        owners,
                        chat_hosts,
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
                        chat_hosts: HashMap::new(),
                        trusted: false,
                    }
                }
            };
            census.insert(kernel.clone(), view);
        }
        census
    }

    async fn reconcile_chat_hosts(self: &Arc<Self>, census: &HashMap<KernelId, RuntimeCensus>) {
        let Some(store) = self.chat_hosts.clone() else {
            return;
        };
        let statuses = vec![
            crate::ChatHostStatus::Provisioning,
            crate::ChatHostStatus::Active,
            crate::ChatHostStatus::Pausing,
            crate::ChatHostStatus::Paused,
            crate::ChatHostStatus::Reviving,
        ];

        // Prove the complete bounded walk is readable before any host mutation.
        let mut cursor = None;
        loop {
            let query = crate::ChatHostListQuery::new(
                PageRequest::new(crate::store::MAX_PAGE_SIZE, cursor),
                statuses.clone(),
            );
            match store.list_candidates(&query).await {
                Ok(page) => {
                    cursor = page.next;
                    if cursor.is_none() {
                        break;
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "chat host reconciliation skipped: store preflight failed");
                    return;
                }
            }
        }

        let mut cursor = None;
        loop {
            let query = crate::ChatHostListQuery::new(
                PageRequest::new(crate::store::MAX_PAGE_SIZE, cursor),
                statuses.clone(),
            );
            let page = match store.list_candidates(&query).await {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(%error, "chat host reconciliation stopped: store scan failed");
                    return;
                }
            };
            for host in page.items {
                self.settle_chat_host(host, census).await;
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
    }

    async fn settle_chat_host(
        self: &Arc<Self>,
        observed: crate::ChatHostRecord,
        census: &HashMap<KernelId, RuntimeCensus>,
    ) {
        if self.chat_hosts_starting.read().await.contains(&observed.id) {
            return;
        }
        let Some(view) = census.get(&observed.kernel) else {
            return;
        };
        if !view.trusted {
            return;
        }
        let Some(store) = self.chat_hosts.clone() else {
            return;
        };
        let observed_generation = crate::chat::generation_for(&observed);
        let observed_runtime_is_serving = observed.runtime_id.as_ref().is_some_and(|runtime| {
            view.serving.contains(runtime)
                && view.chat_hosts.get(runtime) == Some(&observed.id.0)
                && view.generations.get(runtime)
                    == Some(&layer3_adapter::RuntimeGeneration::Recorded(
                        observed_generation,
                    ))
        });
        if observed.status == crate::ChatHostStatus::Active
            && observed_runtime_is_serving
            && self.runtime_version.is_some()
            && observed.runtime_version != self.runtime_version
        {
            if self
                .chat_hosts_starting
                .write()
                .await
                .insert(observed.id.clone())
            {
                let id = observed.id.clone();
                if let Err(error) = self.rotate_chat_host_on_image_drift(&id).await {
                    tracing::warn!(chat_host = %id, %error, "chat image rotation remains pending");
                }
                self.chat_hosts_starting.write().await.remove(&id);
            }
            return;
        }
        if observed.status == crate::ChatHostStatus::Pausing
            && observed.revive_cause == Some(crate::ChatReviveCause::ImageDrift)
        {
            if self
                .chat_hosts_starting
                .write()
                .await
                .insert(observed.id.clone())
            {
                let id = observed.id.clone();
                let old_runtime_is_serving = observed.runtime_id.as_ref().is_some_and(|runtime| {
                    view.serving.contains(runtime)
                        && view.chat_hosts.get(runtime) == Some(&observed.id.0)
                        && view.generations.get(runtime)
                            == Some(&layer3_adapter::RuntimeGeneration::Recorded(
                                observed_generation,
                            ))
                });
                let outcome = if old_runtime_is_serving {
                    self.resume_chat_image_rotation(&id).await
                } else if observed.checkpoint.as_ref().is_some_and(|checkpoint| {
                    checkpoint.checkpoint_version == observed.checkpoint_sequence
                }) {
                    self.finish_torn_down_chat_image_rotation(&id).await
                } else {
                    Err(crate::CoreError::RuntimeUnavailable(
                        "chat image rotation lost its runtime before publishing the held checkpoint"
                            .into(),
                    ))
                };
                if let Err(error) = outcome {
                    tracing::warn!(chat_host = %id, %error, "chat image-rotation claim remains pending");
                }
                self.chat_hosts_starting.write().await.remove(&id);
            }
            return;
        }
        if matches!(
            observed.status,
            crate::ChatHostStatus::Pausing | crate::ChatHostStatus::Paused
        ) && observed
            .last_transition_claim
            .as_deref()
            .is_some_and(|claim| claim.starts_with("chrt_"))
        {
            if self
                .chat_hosts_starting
                .write()
                .await
                .insert(observed.id.clone())
            {
                let id = observed.id.clone();
                let old_runtime_is_serving = observed.runtime_id.as_ref().is_some_and(|runtime| {
                    view.serving.contains(runtime)
                        && view.chat_hosts.get(runtime) == Some(&observed.id.0)
                        && view.generations.get(runtime)
                            == Some(&layer3_adapter::RuntimeGeneration::Recorded(
                                observed_generation,
                            ))
                });
                let exact_checkpoint_is_published = observed.status
                    == crate::ChatHostStatus::Paused
                    && observed.checkpoint.as_ref().is_some_and(|checkpoint| {
                        checkpoint.checkpoint_version == observed.checkpoint_sequence
                    });
                let outcome = if observed.runtime_id.is_none() || old_runtime_is_serving {
                    self.resume_chat_retirement_claim(&id).await
                } else if exact_checkpoint_is_published {
                    self.finish_torn_down_chat_retirement(&id).await
                } else {
                    let lost_runtime = observed.runtime_id.as_deref().unwrap_or_default();
                    let cause = observed
                        .runtime_id
                        .as_ref()
                        .and_then(|runtime| view.stopped_because.get(runtime))
                        .cloned()
                        .map(RuntimeLossCause::from);
                    self.recover_interrupted_chat_pause(&id, lost_runtime, cause)
                        .await
                };
                if let Err(error) = outcome {
                    tracing::warn!(chat_host = %id, %error, "chat retirement claim remains pending");
                }
                self.chat_hosts_starting.write().await.remove(&id);
            }
            return;
        }
        if matches!(
            observed.status,
            crate::ChatHostStatus::Pausing | crate::ChatHostStatus::Paused
        ) && observed.revive_cause.is_none()
            && observed.last_transition_claim.is_some()
        {
            if self
                .chat_hosts_starting
                .write()
                .await
                .insert(observed.id.clone())
            {
                let id = observed.id.clone();
                let old_runtime_is_serving = observed.runtime_id.as_ref().is_some_and(|runtime| {
                    view.serving.contains(runtime)
                        && view.chat_hosts.get(runtime) == Some(&observed.id.0)
                        && view.generations.get(runtime)
                            == Some(&layer3_adapter::RuntimeGeneration::Recorded(
                                observed_generation,
                            ))
                });
                let exact_checkpoint_is_published = observed.status
                    == crate::ChatHostStatus::Paused
                    && observed.checkpoint.as_ref().is_some_and(|checkpoint| {
                        checkpoint.checkpoint_version == observed.checkpoint_sequence
                    });
                let outcome = if old_runtime_is_serving {
                    self.resume_chat_pause_claim(&id).await
                } else if exact_checkpoint_is_published {
                    self.finish_torn_down_chat_pause(&id).await
                } else {
                    let lost_runtime = observed.runtime_id.as_deref().unwrap_or_default();
                    let cause = observed
                        .runtime_id
                        .as_ref()
                        .and_then(|runtime| view.stopped_because.get(runtime))
                        .cloned()
                        .map(RuntimeLossCause::from);
                    self.recover_interrupted_chat_pause(&id, lost_runtime, cause)
                        .await
                };
                if let Err(error) = outcome {
                    tracing::warn!(chat_host = %id, %error, "chat inactivity-pause claim remains pending");
                }
                self.chat_hosts_starting.write().await.remove(&id);
            }
            return;
        }
        let matching_chat_runtime_exists = view.chat_hosts.iter().any(|(runtime, host)| {
            host == observed.id.as_str()
                && view.serving.contains(runtime)
                && view.generations.get(runtime)
                    == Some(&layer3_adapter::RuntimeGeneration::Recorded(
                        observed_generation,
                    ))
        });
        if observed.status == crate::ChatHostStatus::Active
            && !matching_chat_runtime_exists
            && observed
                .runtime_id
                .as_ref()
                .is_some_and(|runtime| !view.serving.contains(runtime))
        {
            if self
                .chat_hosts_starting
                .write()
                .await
                .insert(observed.id.clone())
            {
                let id = observed.id.clone();
                let runtime = observed
                    .runtime_id
                    .clone()
                    .expect("the lost-runtime predicate proved a handle exists");
                let cause = view
                    .stopped_because
                    .get(&runtime)
                    .cloned()
                    .map(RuntimeLossCause::from);
                if let Err(error) = self.recover_lost_chat_host(&id, &runtime, cause).await {
                    tracing::warn!(chat_host = %id, %error, "chat runtime-loss recovery remains pending");
                }
                self.chat_hosts_starting.write().await.remove(&id);
            }
            return;
        }
        if observed.status == crate::ChatHostStatus::Reviving {
            let _owner_guard = self.chat_host_guards.lock(&observed.owner).await;
            let mut current = match store.get(&observed.id).await {
                Ok(Some(current))
                    if current.status == crate::ChatHostStatus::Reviving
                        && crate::chat::generation_for(&current) == observed_generation
                        && current.last_transition_claim == observed.last_transition_claim =>
                {
                    current
                }
                _ => return,
            };
            let mut candidates = view
                .chat_hosts
                .iter()
                .filter(|&(runtime, host)| {
                    host == current.id.as_str()
                        && view.serving.contains(runtime)
                        && view.generations.get(runtime)
                            == Some(&layer3_adapter::RuntimeGeneration::Recorded(
                                observed_generation,
                            ))
                })
                .map(|(runtime, _)| RuntimeHandle(runtime.clone()))
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| left.0.cmp(&right.0));
            if let Some(recorded) = current.runtime_id.as_deref() {
                if let Some(index) = candidates
                    .iter()
                    .position(|candidate| candidate.0 == recorded)
                {
                    candidates.swap(0, index);
                }
            }
            let chosen = candidates.first().cloned();
            if let Some(authoritative) = chosen.as_ref() {
                self.cleanup_duplicate_chat_runtimes(&current, &candidates, authoritative)
                    .await;
            }
            if current.runtime_id.as_deref()
                != chosen.as_ref().map(|candidate| candidate.0.as_str())
            {
                if let Some(stale) = current.runtime_id.as_ref() {
                    let Some(adapter) = self.adapters.get(&current.kernel) else {
                        return;
                    };
                    if adapter
                        .terminate_if_identity(&RuntimeIdentity {
                            handle: RuntimeHandle(stale.clone()),
                            session: current.id.0.clone(),
                            generation: Some(crate::chat::generation_for(&current)),
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                current.runtime_id = chosen.as_ref().map(|candidate| candidate.0.clone());
            }
            settle_expired_chat_dispatch_fences(&mut current);
            current.updated_at = crate::clock::now_rfc3339();
            let settled = match store.update_if_revision(current.revision, &current).await {
                Ok(Some(settled)) => settled,
                _ => return,
            };
            self.spawn_chat_revival(settled.id);
            return;
        }
        let _owner_guard = self.chat_host_guards.lock(&observed.owner).await;
        let mut current = match store.get(&observed.id).await {
            Ok(Some(current)) => current,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(chat_host = %observed.id, %error, "cannot refresh chat host during reconciliation");
                return;
            }
        };
        if self.chat_hosts_starting.read().await.contains(&current.id) {
            return;
        }
        let expected_generation = crate::chat::generation_for(&current);
        let mut candidates = view
            .chat_hosts
            .iter()
            .filter(|&(runtime, host)| {
                host == current.id.as_str()
                    && view.serving.contains(runtime)
                    && view.generations.get(runtime)
                        == Some(&layer3_adapter::RuntimeGeneration::Recorded(
                            expected_generation,
                        ))
            })
            .map(|(runtime, _)| RuntimeHandle(runtime.clone()))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        if let Some(recorded) = current.runtime_id.as_deref() {
            if let Some(index) = candidates
                .iter()
                .position(|candidate| candidate.0 == recorded)
            {
                candidates.swap(0, index);
            }
        }

        if current.status == crate::ChatHostStatus::Active {
            if current.runtime_id.as_ref().is_some_and(|runtime| {
                view.serving.contains(runtime)
                    && view.chat_hosts.get(runtime) == Some(&current.id.0)
                    && view.generations.get(runtime)
                        == Some(&layer3_adapter::RuntimeGeneration::Recorded(
                            expected_generation,
                        ))
            }) {
                let dispatches_settled = settle_expired_chat_dispatch_fences(&mut current);
                if dispatches_settled {
                    current.updated_at = crate::clock::now_rfc3339();
                    current = match store.update_if_revision(current.revision, &current).await {
                        Ok(Some(current)) => current,
                        _ => return,
                    };
                }
                let authoritative = RuntimeHandle(
                    current
                        .runtime_id
                        .clone()
                        .expect("the active-runtime predicate proved an id exists"),
                );
                self.cleanup_duplicate_chat_runtimes(&current, &candidates, &authoritative)
                    .await;
                let dispatch_is_fenced = current.prompt_admissions.iter().any(|admission| {
                    admission.state == crate::ChatPromptDeliveryState::DispatchFenced
                });
                if !dispatch_is_fenced
                    && current
                        .prompt_admissions
                        .iter()
                        .any(|admission| admission.state == crate::ChatPromptDeliveryState::Queued)
                {
                    self.spawn_chat_revival(current.id.clone());
                }
                return;
            }
            if let Some(candidate) = candidates.first().cloned() {
                self.cleanup_duplicate_chat_runtimes(&current, &candidates, &candidate)
                    .await;
                current.status = crate::ChatHostStatus::Provisioning;
                current.runtime_id = Some(candidate.0.clone());
                current.last_transition_claim =
                    Some(format!("chcl_{}", uuid::Uuid::new_v4().simple()));
                current.transition_started_at = Some(crate::clock::now_rfc3339());
                current.updated_at = crate::clock::now_rfc3339();
                let claimed = match store.update_if_revision(current.revision, &current).await {
                    Ok(Some(claimed)) => claimed,
                    _ => return,
                };
                self.finish_adopted_chat_host(claimed, candidate).await;
                return;
            }

            let cleanup_pending = if let Some(runtime) = current.runtime_id.as_ref() {
                let Some(adapter) = self.adapters.get(&current.kernel) else {
                    return;
                };
                adapter
                    .terminate_if_identity(&RuntimeIdentity {
                        handle: RuntimeHandle(runtime.clone()),
                        session: current.id.0.clone(),
                        generation: Some(expected_generation),
                    })
                    .await
                    .is_err()
            } else {
                false
            };
            if !cleanup_pending {
                current.runtime_id = None;
            }
            let now = crate::clock::now_rfc3339();
            current.status = crate::ChatHostStatus::Paused;
            current.paused_at = Some(now.clone());
            current.last_transition_claim = None;
            current.transition_started_at = None;
            current.last_failure = Some(crate::ChatHostFailure {
                phase: crate::ChatProvisioningPhase::RuntimeLost,
                command_index: None,
                retryable: true,
                cleanup_pending,
                occurred_at: now.clone(),
            });
            current.updated_at = now;
            let _ = store.update_if_revision(current.revision, &current).await;
            return;
        }

        if current.status != crate::ChatHostStatus::Provisioning {
            return;
        }
        if current
            .last_failure
            .as_ref()
            .is_some_and(|failure| !failure.retryable)
        {
            return;
        }

        let chosen = current
            .runtime_id
            .as_ref()
            .filter(|runtime| {
                view.serving.contains(*runtime)
                    && view.chat_hosts.get(*runtime) == Some(&current.id.0)
                    && view.generations.get(*runtime)
                        == Some(&layer3_adapter::RuntimeGeneration::Recorded(
                            expected_generation,
                        ))
            })
            .cloned()
            .map(RuntimeHandle)
            .or_else(|| candidates.first().cloned());
        if let Some(authoritative) = chosen.as_ref() {
            self.cleanup_duplicate_chat_runtimes(&current, &candidates, authoritative)
                .await;
        }
        if chosen.is_none() {
            if let Some(runtime) = current.runtime_id.clone() {
                let Some(adapter) = self.adapters.get(&current.kernel) else {
                    return;
                };
                if adapter
                    .terminate_if_identity(&RuntimeIdentity {
                        handle: RuntimeHandle(runtime),
                        session: current.id.0.clone(),
                        generation: Some(expected_generation),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                current.runtime_id = None;
            }
        }

        current.last_transition_claim = Some(format!("chcl_{}", uuid::Uuid::new_v4().simple()));
        current.transition_started_at = Some(crate::clock::now_rfc3339());
        current.updated_at = crate::clock::now_rfc3339();
        let claimed = match store.update_if_revision(current.revision, &current).await {
            Ok(Some(claimed)) => claimed,
            _ => return,
        };
        if let Some(candidate) = chosen {
            if claimed.runtime_id.as_deref() != Some(candidate.0.as_str()) {
                let mut adopted = claimed;
                adopted.runtime_id = Some(candidate.0.clone());
                adopted.updated_at = crate::clock::now_rfc3339();
                let adopted = match store.update_if_revision(adopted.revision, &adopted).await {
                    Ok(Some(adopted)) => adopted,
                    _ => return,
                };
                self.finish_adopted_chat_host(adopted, candidate).await;
            } else {
                self.finish_adopted_chat_host(claimed, candidate).await;
            }
        } else {
            if !self
                .chat_hosts_starting
                .write()
                .await
                .insert(claimed.id.clone())
            {
                return;
            }
            let id = claimed.id.clone();
            let _ = self.provision_claimed_chat_host(claimed, None).await;
            self.chat_hosts_starting.write().await.remove(&id);
        }
    }

    async fn cleanup_duplicate_chat_runtimes(
        &self,
        host: &crate::ChatHostRecord,
        candidates: &[RuntimeHandle],
        authoritative: &RuntimeHandle,
    ) {
        let Some(adapter) = self.adapters.get(&host.kernel) else {
            return;
        };
        let identity_generation = Some(crate::chat::generation_for(host));
        for duplicate in candidates
            .iter()
            .filter(|candidate| *candidate != authoritative)
        {
            let identity = RuntimeIdentity {
                handle: duplicate.clone(),
                session: host.id.0.clone(),
                generation: identity_generation,
            };
            if let Err(error) = adapter.terminate_if_identity(&identity).await {
                tracing::warn!(
                    chat_host = %host.id,
                    runtime = %duplicate.0,
                    %error,
                    "a duplicate chat runtime could not be conditionally removed"
                );
            }
        }
    }

    async fn finish_adopted_chat_host(
        self: &Arc<Self>,
        host: crate::ChatHostRecord,
        handle: RuntimeHandle,
    ) {
        if !self
            .chat_hosts_starting
            .write()
            .await
            .insert(host.id.clone())
        {
            return;
        }
        let id = host.id.clone();
        if let Some(adapter) = self.adapters.get(&host.kernel).cloned() {
            let _ = self
                .finish_claimed_chat_host_runtime(host, handle, adapter)
                .await;
        }
        self.chat_hosts_starting.write().await.remove(&id);
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
        let retains_unrecorded_loss = record.session.wants_runtime()
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
            Ok(Some(record)) if matches!(&record.skill_packs, SkillPackState::StagedSummary) => {
                tracing::error!(
                    session_id = %id,
                    "exact cleanup read returned a projected skill custody summary"
                );
                return None;
            }
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

        // Store listings may deliberately redact staged skill bodies so one
        // reconciliation page cannot retain hundreds of multi-megabyte rows.
        // No provider or durable mutation may inherit that projection: reload
        // the exact row before any settlement decision and refuse the sweep if
        // the point-read contract is not exact.
        if matches!(&record.skill_packs, SkillPackState::StagedSummary) {
            record = match self.store.get(&id).await {
                Ok(Some(exact)) if !matches!(&exact.skill_packs, SkillPackState::StagedSummary) => {
                    exact
                }
                Ok(Some(_)) => {
                    tracing::error!(
                        session_id = %id,
                        "exact session read returned a projected skill custody summary"
                    );
                    reclaimed.deferred_sessions += 1;
                    return;
                }
                Ok(None) | Err(StoreError::SessionDeleted { .. }) => return,
                Err(error) => {
                    tracing::warn!(
                        session_id = %id,
                        %error,
                        "cannot reload exact staged skill custody before reconciliation"
                    );
                    reclaimed.deferred_sessions += 1;
                    return;
                }
            };
            if !record.session.wants_runtime() {
                return;
            }
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

        // A failed post-publication claim clear leaves a usable local entry
        // deliberately fenced. Retry that exact CAS before the ordinary
        // "already served here" return. An outgoing or otherwise stale entry
        // cannot prove publication. Its removal must still acquire a fresh
        // exact lease: the cached claim/authority can become H1 while a local
        // concurrent handover has already published H2.
        if let Some(claim) = record.rotation_claim.clone() {
            let expected_generation = coordinated_authority
                .as_ref()
                .map(|authority| authority.generation);
            let local = {
                let entries = self.entries.read().await;
                entries.get(&id).map(|entry| {
                    (
                        entry.runtime.as_ref().map(|runtime| runtime.0.clone()),
                        entry.runtime_generation,
                        entry.session.status,
                        entry.publication_complete,
                    )
                })
            };
            if let Some((runtime_id, runtime_generation, status, complete)) = local {
                let exact_publication = complete
                    && runtime_id == record.runtime_id
                    && expected_generation
                        .is_none_or(|generation| generation == runtime_generation)
                    && !status.is_rotating();
                if exact_publication {
                    let finalization_lease = match self
                        .acquire_runtime_lease_at(
                            &record,
                            crate::coordination::SessionOperation::Readopt,
                        )
                        .await
                    {
                        Ok(lease) => lease,
                        Err(error) => {
                            tracing::debug!(
                                session_id = %id,
                                %error,
                                "the rotation publication retry could not acquire exact authority"
                            );
                            reclaimed.deferred_sessions += 1;
                            return;
                        }
                    };
                    let finalized = self.finalize_rotation_publication(&id, &claim).await;
                    if let Some(lease) = finalization_lease {
                        if let Err(error) = self.store.release_session_lease(&lease).await {
                            tracing::debug!(
                                session_id = %id,
                                %error,
                                "the rotation publication retry lease ended before explicit release"
                            );
                        }
                    }
                    match finalized {
                        Ok(true) => return,
                        Ok(false) => {
                            reclaimed.deferred_sessions += 1;
                            return;
                        }
                        Err(error) => {
                            tracing::warn!(
                                session_id = %id,
                                %error,
                                "the published runtime remains fenced by its rotation claim"
                            );
                            reclaimed.deferred_sessions += 1;
                            return;
                        }
                    }
                }

                if coordinated_authority.is_some() {
                    // Re-read the durable boundary and remove only while a
                    // fresh Reclaim lease plus the local lifecycle/input gates
                    // exclude a successor publication. A following sweep can
                    // then readopt from one coherent snapshot.
                    self.discard_stale_coordinated_entry(&record).await;
                    reclaimed.deferred_sessions += 1;
                    return;
                }
            }
        }

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
            let has_local_entry = {
                let entries = self.entries.read().await;
                match entries.get(&id) {
                    Some(entry)
                        if entry.publication_complete
                            && entry.runtime_generation == authority.generation
                            && entry.runtime.as_ref().map(|runtime| runtime.0.as_str())
                                == authority.runtime_id.as_deref() =>
                    {
                        return;
                    }
                    Some(_) => true,
                    None => false,
                }
            };
            if has_local_entry {
                // The authority snapshot above can become stale while a local
                // readoption publishes. Recheck and remove only under a fresh
                // exact lease, then let the next sweep readopt from one coherent
                // record/authority snapshot.
                self.discard_stale_coordinated_entry(&record).await;
                reclaimed.deferred_sessions += 1;
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
            let expected_runtime = record.runtime_id.clone();
            let expected_boundary = record.clone();
            // Nothing in this critical section may reacquire this session's
            // lifecycle gate. It deliberately spans the authoritative store
            // recheck, provider teardown, loss persistence, and exact entry
            // removal so delivery, rotation, and reconciliation cannot
            // interleave generations.
            let _lifecycle = self.lock_lifecycle(&id).await;
            let _input_fence = self.input_gates.gate(&id).write_owned().await;
            let latest = match self.store.get(&id).await {
                Ok(Some(latest))
                    if latest.session.wants_runtime()
                        && same_compatibility_runtime_boundary(&expected_boundary, &latest) =>
                {
                    latest
                }
                Ok(_) => {
                    tracing::debug!(session_id = %id, "the session changed before its lost runtime could be recorded");
                    reclaimed.deferred_sessions += 1;
                    return;
                }
                Err(error) => {
                    tracing::warn!(session_id = %id, %error, "cannot recheck the session before recording its lost runtime");
                    reclaimed.deferred_sessions += 1;
                    return;
                }
            };
            record = latest;

            let entry_matches_snapshot = {
                let entries = self.entries.read().await;
                entries.get(&id).is_none_or(|entry| {
                    entry.runtime_generation == 0
                        && entry.runtime.as_ref().map(|handle| handle.0.as_str())
                            == expected_runtime.as_deref()
                })
            };
            if !entry_matches_snapshot {
                tracing::warn!(session_id = %id, "a newer runtime entry replaced the one reported lost; deferring reconciliation");
                reclaimed.deferred_sessions += 1;
                return;
            }

            if let Some(runtime) = expected_runtime.as_deref() {
                let view = census
                    .get(&record.session.kernel)
                    .expect("the trusted census selected this compatibility session");
                if view.owners.contains_key(runtime) {
                    let Some(adapter) = self.adapters.get(&record.session.kernel) else {
                        tracing::warn!(
                            session_id = %id,
                            kernel = %record.session.kernel,
                            "cannot tear down a listed non-serving runtime without its adapter"
                        );
                        reclaimed.deferred_sessions += 1;
                        return;
                    };
                    if let Err(error) = self
                        .require_compatibility_provider_mutation(
                            &id,
                            "non-serving runtime reconciliation",
                        )
                        .await
                    {
                        tracing::warn!(
                            session_id = %id,
                            runtime = %runtime,
                            %error,
                            "cannot authorize non-serving compatibility runtime teardown"
                        );
                        reclaimed.deferred_sessions += 1;
                        return;
                    }
                    if let Err(error) = adapter.terminate(&RuntimeHandle(runtime.to_string())).await
                    {
                        tracing::warn!(
                            session_id = %id,
                            runtime = %runtime,
                            %error,
                            "non-serving compatibility runtime teardown failed"
                        );
                        reclaimed.deferred_sessions += 1;
                        return;
                    }
                }
                let cause = view
                    .stopped_because
                    .get(runtime)
                    .copied()
                    .map(RuntimeLossCause::from);
                if let Err(error) = self.record_runtime_lost(&mut record, runtime, cause).await {
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

            // Remove only the entry backed by the runtime the census proved
            // lost. A newer generation is evidence that this sweep snapshot
            // is stale and must never be discarded here.
            let removed = {
                let mut entries = self.entries.write().await;
                let exact_lost = entries.get(&id).is_some_and(|entry| {
                    entry.runtime.as_ref().map(|handle| handle.0.as_str())
                        == expected_runtime.as_deref()
                });
                exact_lost.then(|| entries.remove(&id)).flatten()
            };
            if removed.is_some() {
                if expected_runtime.is_some() {
                    tracing::warn!(session_id = %id, "the runtime behind a live session has gone");
                }
            } else if self.entries.read().await.contains_key(&id) {
                tracing::warn!(session_id = %id, "the runtime entry changed while its loss was being recorded; deferring reconciliation");
                reclaimed.deferred_sessions += 1;
                return;
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

        let (has_local_entry, stale_compatibility_entry) = {
            let entries = self.entries.read().await;
            match entries.get(&id) {
                Some(entry) => (
                    true,
                    entry.session.status.is_rotating()
                        || !entry.publication_complete
                        || entry.runtime_generation != 0
                        || entry.runtime.as_ref().map(|runtime| runtime.0.as_str())
                            != record.runtime_id.as_deref(),
                ),
                None => (false, false),
            }
        };
        if stale_compatibility_entry {
            self.discard_stale_compatibility_entry(&record).await;
            reclaimed.deferred_sessions += 1;
            return;
        }
        if has_local_entry {
            if alive != Some(false) {
                // Either it is running, or nobody can say otherwise. Neither is
                // grounds for touching a session this process is serving.
                return;
            }
            // The known-lost branch above removes only the exact generation it
            // proved dead. Anything still here is newer or internally
            // inconsistent; neither may be replaced from this stale snapshot.
            reclaimed.deferred_sessions += 1;
            return;
        }

        // Worth trying whenever the runtime is not known to be gone, because
        // attaching settles the question that the census could not: a sandbox
        // that answers is running, whatever anything else says.
        if record.runtime_id.is_some() && alive != Some(false) {
            // The resume identity and pending approvals are both part of the
            // attachment's live state. Read them before touching the runtime,
            // and defer when persistence cannot supply the complete history:
            // replacing a runtime merely because its state could not be rebuilt
            // would abandon the conversation the provider still reports alive.
            let readoption = match self.prepare_readoption(&record).await {
                Ok(Some(readoption)) => readoption,
                Ok(None) => return,
                Err(error) => {
                    tracing::warn!(
                        session_id = %id,
                        %error,
                        "cannot rebuild conversation state; leaving the live runtime untouched"
                    );
                    reclaimed.deferred_sessions += 1;
                    return;
                }
            };
            match self.readopt_prepared(&record, readoption).await {
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
            // termination, or successor runtime. Retire only the same fresh
            // compatibility boundary, then reprovision from the committed
            // runtime-free result.
            let Some(cleared) = self
                .clear_failed_compatibility_readoption(&record, reclaimed)
                .await
            else {
                return;
            };
            record = cleared;
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
                let confirmed = self.confirm_retired_lifecycle_authority(&mut lease).await;
                tracing::warn!(
                    session_id = %id,
                    runtime = %runtime,
                    %error,
                    confirmed,
                    "cannot retire the lost runtime authority"
                );
                // Whether retirement committed is unknown until confirmation.
                // In either case this replica's old channel is no longer safe:
                // a confirmed retirement has no receipt to continue with, and
                // an unconfirmed one may still have committed remotely.
                self.entries.write().await.remove(&id);
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
        let expected_recovery = starting
            .pending_handover
            .as_ref()
            .and_then(|pending| pending.trigger_recovery.as_ref());
        let trigger_recovery = match self
            .resolve_trigger_recovery_pair_for(&starting, expected_recovery)
            .await
        {
            Ok(pair) => pair,
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "Trigger recovery pair verification failed; leaving replacement fenced"
                );
                self.finish_failed_coordinated_recovery(&id, &lease, false, reclaimed)
                    .await;
                return;
            }
        };
        if starting.trigger_origin.is_some() && trigger_recovery.is_none() {
            tracing::warn!(
                session_id = %id,
                "Trigger runtime recovery has no complete workspace/conversation pair"
            );
            self.finish_failed_coordinated_recovery(&id, &lease, false, reclaimed)
                .await;
            return;
        }
        if starting.session.status.is_rotating() && starting.rotation_claim.is_none() {
            starting.rotation_claim = Some(format!("rotn_{}", uuid::Uuid::new_v4().simple()));
        }
        if starting.rotation_claim.is_some() {
            self.rotations_in_progress.write().await.insert(id.clone());
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
            if let Err(error) = self.advance_runtime_lease(&mut lease, &boundary).await {
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
        let rotation_claim = starting.rotation_claim.clone();
        let spec = SessionSpec {
            surface: None,
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
        let trigger_origin = starting.trigger_origin.clone();
        let session = starting.session;
        tokio::spawn(async move {
            service
                .bring_up_with_lease(
                    session,
                    adapter,
                    spec,
                    geometry,
                    None,
                    crate::provision_support::BringUpContext {
                        outcome: crate::provision_failure::Abandonment::Keep,
                        lease: Some(lease),
                        handover: None,
                        trigger_recovery,
                        trigger_origin,
                        prepared_skills: None,
                        rotation_claim,
                    },
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
    /// Ordinary sessions move to `Recovering` and the same pass starts a
    /// replacement runtime. A Trigger session without a verified handover
    /// snapshot instead moves to a non-interactive degraded boundary. Starting
    /// a new agent conversation there would preserve the session id while
    /// silently discarding its turn context.
    async fn record_runtime_lost(
        &self,
        record: &mut SessionRecord,
        runtime: &str,
        cause: Option<RuntimeLossCause>,
    ) -> Result<(), StoreError> {
        let id = record.session.id.clone();
        let has_trigger_recovery =
            if record.trigger_origin.is_some() && record.pending_handover.is_none() {
                match self.resolve_trigger_recovery_pair_for(record, None).await {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(error) => {
                        tracing::warn!(
                            session_id = %id,
                            %error,
                            "the lost Trigger runtime has no presently verifiable recovery pair"
                        );
                        false
                    }
                }
            } else {
                false
            };
        let previous = record.runtime_id.take();
        let previous_state = record.session.recovery_state;
        let previous_status = record.session.status;
        let previous_initial_prompt = record.initial_prompt.clone();
        let requires_explicit_recovery = record.trigger_origin.is_some()
            && record.pending_handover.is_none()
            && !has_trigger_recovery;
        let mut events = vec![Event {
            session_id: id.clone(),
            seq: 0,
            kind: EventKind::RuntimeLost {
                runtime_id: runtime.to_string(),
                cause,
            },
            appended_at: String::new(),
        }];
        if requires_explicit_recovery {
            record.session.status = SessionStatus::Rotating;
            record.session.recovery_state = RecoveryState::Degraded;
            if let Some(initial) = record.initial_prompt.as_mut() {
                if matches!(
                    initial.delivery.state,
                    crate::PromptDeliveryState::DispatchFenced
                        | crate::PromptDeliveryState::WriteConfirmed
                        | crate::PromptDeliveryState::ExecutionObserved
                ) {
                    initial.delivery.state = crate::PromptDeliveryState::RecoveryRequired;
                    initial.delivery.updated_at = crate::clock::now_rfc3339();
                    events.push(Event {
                        session_id: id.clone(),
                        seq: 0,
                        kind: EventKind::InitialPromptDeliveryChanged {
                            state: crate::PromptDeliveryState::RecoveryRequired,
                        },
                        appended_at: String::new(),
                    });
                }
            }
            events.push(Event {
                session_id: id.clone(),
                seq: 0,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Rotating,
                    stop_reason: None,
                    detail: Some(
                        "runtime continuity could not be verified; explicit recovery is required"
                            .into(),
                    ),
                    capability: None,
                },
                appended_at: String::new(),
            });
        } else {
            record.session.recovery_state = RecoveryState::Recovering;
        }
        if let Err(e) = self.persist_durably(record, events).await {
            // Nothing was recorded, so nothing is claimed. Put the local copy
            // back and let a later sweep establish the loss again.
            record.runtime_id = previous;
            record.session.recovery_state = previous_state;
            record.session.status = previous_status;
            record.initial_prompt = previous_initial_prompt;
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
    async fn reprovision(self: &Arc<Self>, mut record: SessionRecord, reclaimed: &mut Reclaimed) {
        let id = record.session.id.clone();
        if record.runtime_id.is_some() {
            tracing::error!(
                session_id = %id,
                runtime = ?record.runtime_id,
                "refusing to reprovision while a previous runtime remains durable"
            );
            reclaimed.deferred_sessions += 1;
            return;
        }
        let Some(adapter) = self.adapters.get(&record.session.kernel).cloned() else {
            return;
        };
        if !self.begin_start(&id).await {
            return;
        }
        let (restore, trigger_recovery) = if record.pending_handover.is_some() {
            let recovery = match self.load_pending_handover_recovery(&record).await {
                Ok(recovery) => recovery,
                Err(error) => {
                    tracing::warn!(
                        session_id = %id,
                        %error,
                        "cannot verify the pending handover snapshot; leaving the session fenced"
                    );
                    self.finish_start(&id).await;
                    reclaimed.deferred_sessions += 1;
                    return;
                }
            };
            record = match self.publish_pending_handover_target(record).await {
                Ok(published) => published,
                Err(error) => {
                    tracing::warn!(
                        session_id = %id,
                        %error,
                        "cannot publish the pending handover target; leaving the session fenced"
                    );
                    self.finish_start(&id).await;
                    reclaimed.deferred_sessions += 1;
                    return;
                }
            };
            (recovery.restore, recovery.trigger_recovery)
        } else if record.is_effectively_rotating() {
            let trigger_recovery = match self.resolve_trigger_recovery_pair_for(&record, None).await
            {
                Ok(Some(pair)) if record.trigger_origin.is_some() => Some(pair),
                Ok(_) => {
                    tracing::error!(
                        session_id = %id,
                        "refusing to provision a rotating session without recoverable conversation authority"
                    );
                    self.finish_start(&id).await;
                    reclaimed.deferred_sessions += 1;
                    return;
                }
                Err(error) => {
                    tracing::warn!(
                        session_id = %id,
                        %error,
                        "cannot verify the Trigger recovery pair; leaving the session fenced"
                    );
                    self.finish_start(&id).await;
                    reclaimed.deferred_sessions += 1;
                    return;
                }
            };
            (None, trigger_recovery)
        } else {
            let trigger_recovery = match self.resolve_trigger_recovery_pair_for(&record, None).await
            {
                Ok(pair) => pair,
                Err(error) => {
                    tracing::warn!(session_id = %id, %error, "cannot verify the recovery pair");
                    self.finish_start(&id).await;
                    reclaimed.deferred_sessions += 1;
                    return;
                }
            };
            if record.trigger_origin.is_some() && trigger_recovery.is_none() {
                tracing::warn!(
                    session_id = %id,
                    "Trigger runtime recovery has no complete workspace/conversation pair"
                );
                self.finish_start(&id).await;
                reclaimed.deferred_sessions += 1;
                return;
            }
            (None, trigger_recovery)
        };
        let was_rotating = record.is_effectively_rotating();
        if was_rotating {
            // A replacement process has no in-memory rotation marker to inherit.
            // Preserve the operation's identity before rewriting the durable
            // status to `Provisioning`, through readiness and live installation.
            self.rotations_in_progress.write().await.insert(id.clone());
        }

        let mut starting = record.clone();
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
        let rotation_claim = starting.rotation_claim.clone();
        let spec = SessionSpec {
            surface: None,
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
        let trigger_origin = starting.trigger_origin.clone();
        let session = starting.session;
        tokio::spawn(async move {
            service
                .bring_up(
                    session,
                    spec,
                    adapter,
                    geometry,
                    restore,
                    crate::provision_support::BringUpContext {
                        outcome: crate::provision_failure::Abandonment::Keep,
                        lease: None,
                        handover: None,
                        trigger_recovery,
                        trigger_origin,
                        prepared_skills: None,
                        rotation_claim,
                    },
                )
                .await;
        });
        reclaimed.reprovisioned_sessions += 1;
    }
}

fn settle_expired_chat_dispatch_fences(host: &mut crate::ChatHostRecord) -> bool {
    let now = crate::seconds_from_rfc3339(&crate::clock::now_rfc3339()).unwrap_or(u64::MAX);
    let mut changed = false;
    for admission in &mut host.prompt_admissions {
        if admission.state != crate::ChatPromptDeliveryState::DispatchFenced {
            continue;
        }
        let expired = admission
            .dispatch_fenced_at
            .as_deref()
            .and_then(crate::seconds_from_rfc3339)
            .is_none_or(|fenced_at| {
                now >= fenced_at.saturating_add(CHAT_DISPATCH_FENCE_TIMEOUT_SECONDS)
            });
        if expired {
            admission.state = crate::ChatPromptDeliveryState::RecoveryRequired;
            admission.failure_phase = Some(crate::ChatRevivePhase::PromptDelivery);
            admission.failure_provisioning_phase = None;
            admission.failure_retryable = false;
            changed = true;
        }
    }
    changed
}
