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

use crate::ids::KernelId;
use crate::service::SessionService;
use crate::session::{SessionSpec, SessionStatus};
use crate::store::SessionRecord;

/// What a sweep did, for logging and for tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reclaimed {
    /// Runtimes torn down because no session was entitled to them.
    pub orphaned_runtimes: usize,
    /// Live runtimes this process took back over.
    pub readopted_sessions: usize,
    /// Sessions given a new runtime because theirs was gone.
    pub reprovisioned_sessions: usize,
}

/// What the provider says is running, for one kernel.
///
/// The `trusted` flag is the important part. An enumeration that failed is not
/// evidence that nothing is running, and treating it as such would tear down
/// every live session the moment the provider had a bad minute.
struct RuntimeCensus {
    /// Runtime ids the provider reports as existing.
    handles: HashSet<String>,
    /// Runtime id to the session it says it belongs to.
    owners: HashMap<String, Option<String>>,
    trusted: bool,
}

impl SessionService {
    /// Compare the provider's runtimes against the record, and settle both.
    pub async fn reconcile(self: &Arc<Self>) -> Reclaimed {
        let mut reclaimed = Reclaimed::default();

        let records = match self.store.list_all().await {
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
            .filter(|r| r.session.status.wants_runtime())
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
                    let mut handles = HashSet::new();
                    for (handle, session) in runtimes {
                        handles.insert(handle.0.clone());
                        owners.insert(handle.0, session);
                    }
                    RuntimeCensus {
                        handles,
                        owners,
                        trusted: true,
                    }
                }
                Err(e) => {
                    // Not knowing is not the same as nothing being there.
                    tracing::warn!(kernel = %kernel, error = %e, "cannot enumerate runtimes");
                    RuntimeCensus {
                        handles: HashSet::new(),
                        owners: HashMap::new(),
                        trusted: false,
                    }
                }
            };
            census.insert(kernel.clone(), view);
        }
        census
    }

    /// Give one session whatever it is missing.
    async fn settle_session(
        self: &Arc<Self>,
        record: SessionRecord,
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
            Some(view) if view.trusted => Some(
                record
                    .runtime_id
                    .as_deref()
                    .is_some_and(|r| view.handles.contains(r)),
            ),
            Some(_) => None,
            None => {
                tracing::warn!(session_id = %id, kernel = %record.session.kernel, "no runtime for this session's kernel");
                return;
            }
        };

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
                Err(e) => {
                    // Listed but unreachable is no more usable than gone.
                    tracing::warn!(session_id = %id, error = %e, "cannot take over the runtime");
                }
            }
        }

        self.reprovision(record, reclaimed).await;
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
        if let Err(e) = self.store.upsert(&starting).await {
            tracing::error!(session_id = %id, error = %e, "cannot record the restart");
            self.finish_start(&id).await;
            return;
        }

        tracing::warn!(session_id = %id, "starting a new runtime for a session that lost one");
        let geometry = starting.session.geometry;
        let spec = SessionSpec {
            kernel: starting.session.kernel.clone(),
            geometry: Some(geometry),
            options: starting.session.options.clone(),
            // Nobody is here to delegate one; the renewal loop supplies it.
            subject_credential: None,
        };
        let service = Arc::clone(self);
        let session = starting.session;
        tokio::spawn(async move {
            service.bring_up(session, spec, adapter, geometry).await;
        });
        reclaimed.reprovisioned_sessions += 1;
    }
}
