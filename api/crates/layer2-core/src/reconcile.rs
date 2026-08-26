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
use std::sync::Arc;

use layer3_adapter::RuntimeHandle;

use crate::event::{Event, EventKind};
use crate::ids::KernelId;
use crate::service::SessionService;
use crate::session::{RecoveryState, RuntimeLossCause, SessionSpec, SessionStatus};
use crate::store::{SessionRecord, StoreError};

/// What a sweep did, for logging and for tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reclaimed {
    /// Runtimes torn down because no session was entitled to them.
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
    /// Reclaiming walks `owners` rather than this, so a stopped sandbox that
    /// no session is entitled to is still found and deleted.
    serving: HashSet<String>,
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

        // Only these are entitled to a runtime. A stopped session is not — that
        // is what stopping means — so its sandbox is reclaimed below.
        let entitled: HashSet<String> = records
            .iter()
            .filter(|r| r.session.wants_runtime())
            .map(|r| r.session.id.0.clone())
            .collect();

        for (kernel, view) in &census {
            if !view.trusted {
                continue;
            }
            let Some(adapter) = self.adapters.get(kernel) else {
                continue;
            };
            for (runtime, session) in &view.owners {
                if session.as_deref().is_some_and(|s| entitled.contains(s)) {
                    continue;
                }
                tracing::warn!(
                    runtime = %runtime,
                    session = session.as_deref().unwrap_or("<none>"),
                    "reclaiming a runtime no session is entitled to"
                );
                match adapter.terminate(&RuntimeHandle(runtime.clone())).await {
                    Ok(()) => reclaimed.orphaned_runtimes += 1,
                    Err(e) => {
                        tracing::warn!(runtime = %runtime, error = %e, "could not reclaim runtime")
                    }
                }
            }
        }

        for record in records {
            if !record.session.wants_runtime() {
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
                    let mut serving = HashSet::new();
                    let mut stopped_because = HashMap::new();
                    for listing in runtimes {
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
            let expected_kernel = record.session.kernel.clone();
            let lifecycle = self.lock_lifecycle(&id).await;
            let latest = match self.store.get(&id).await {
                Ok(Some(latest))
                    if latest.session.wants_runtime()
                        && latest.session.kernel == expected_kernel
                        && latest.runtime_id == expected_runtime =>
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
                    entry.runtime.as_ref().map(|handle| handle.0.as_str())
                        == expected_runtime.as_deref()
                })
            };
            if !entry_matches_snapshot {
                tracing::warn!(session_id = %id, "a newer runtime entry replaced the one reported lost; deferring reconciliation");
                reclaimed.deferred_sessions += 1;
                return;
            }

            if let Some(runtime) = expected_runtime.as_deref() {
                let cause = census
                    .get(&record.session.kernel)
                    .and_then(|view| view.stopped_because.get(runtime))
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
            drop(lifecycle);
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
            // termination, deletion, or replacement runtime. Never reprovision
            // from the old sweep snapshot after that: reread the desired state
            // and defer if it now names a different runtime.
            let latest = match self.store.get(&id).await {
                Ok(Some(latest)) if latest.session.wants_runtime() => latest,
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
        let Some(adapter) = self.adapters.get(&record.session.kernel).cloned() else {
            return;
        };
        if !self.begin_start(&id).await {
            return;
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
