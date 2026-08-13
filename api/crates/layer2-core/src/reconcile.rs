//! Reconciling what is running against what is recorded.
//!
//! Sessions do not expire on a timer — an owner decides when their work ends —
//! so this is the only thing in the system that reclaims anything. Everything
//! it recovers comes from one of two disagreements between the record and the
//! provider:
//!
//! A runtime nothing refers to. The control plane can be killed between
//! creating a runtime and recording it, which no amount of ordering inside a
//! single process prevents; a deployment is enough to cause it.
//!
//! A session recorded as starting whose runtime is not coming. The task that
//! was bringing it up died with the process that ran it, and nothing else is
//! going to move it, so it would otherwise sit there indefinitely.

use std::collections::HashSet;
use std::sync::Arc;

use layer3_adapter::RuntimeHandle;

use crate::event::{Event, EventKind};
use crate::service::SessionService;
use crate::session::{RecoveryState, SessionStatus, StopReason};

/// What a sweep did, for logging and for tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reclaimed {
    /// Runtimes torn down because no live session referred to them.
    pub orphaned_runtimes: usize,
    /// Sessions failed because nothing was going to finish starting them.
    pub abandoned_sessions: usize,
}

impl SessionService {
    /// Compare the provider's runtimes against the record, and settle both.
    ///
    /// Deliberately conservative about what it reclaims. A sweep that is wrong
    /// destroys someone's working session, so a runtime is only torn down when
    /// the record positively says it should not exist — never merely because
    /// this process has not heard of it.
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

        // Sessions that are entitled to a runtime. Anything else that exists is
        // either finished or was never recorded.
        let live: HashSet<String> = records
            .iter()
            .filter(|r| !r.session.status.is_terminal())
            .map(|r| r.session.id.0.clone())
            .collect();

        for adapter in self.adapters.values() {
            let runtimes = match adapter.list_runtimes().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "cannot enumerate runtimes for reconciliation");
                    continue;
                }
            };
            for (handle, session) in runtimes {
                let claimed = session.as_deref().is_some_and(|s| live.contains(s));
                if claimed {
                    continue;
                }
                tracing::warn!(
                    runtime = %handle.0,
                    session = session.as_deref().unwrap_or("<none>"),
                    "reclaiming a runtime no live session refers to"
                );
                if let Err(e) = adapter.terminate(&handle).await {
                    tracing::warn!(runtime = %handle.0, error = %e, "could not reclaim runtime");
                    continue;
                }
                reclaimed.orphaned_runtimes += 1;
            }
        }

        // A session still starting, with nothing in this process starting it,
        // is one whose bring-up died with an earlier process.
        let entries = self.entries.read().await;
        let stranded: Vec<_> = records
            .into_iter()
            .filter(|r| r.session.status == SessionStatus::Provisioning)
            .filter(|r| !entries.contains_key(&r.session.id))
            .collect();
        drop(entries);

        for mut record in stranded {
            let id = record.session.id.clone();
            tracing::warn!(session_id = %id, "failing a session nothing is starting");
            let handle = record.runtime_id.take().map(RuntimeHandle);
            record.session.status = SessionStatus::Terminated;
            record.session.stop_reason = Some(StopReason::Error);
            record.session.recovery_state = RecoveryState::Retired;
            record.session.last_seq += 1;
            let seq = record.session.last_seq;

            if let Err(e) = self.store.upsert(&record).await {
                tracing::error!(session_id = %id, error = %e, "cannot record the abandonment");
                continue;
            }
            if let Err(e) = self
                .store
                .append_events(&[Event {
                    session_id: id.clone(),
                    seq,
                    kind: EventKind::StatusChanged {
                        status: SessionStatus::Terminated,
                        stop_reason: Some(StopReason::Error),
                    },
                }])
                .await
            {
                tracing::error!(session_id = %id, error = %e, "cannot record the abandonment event");
            }

            // Its runtime, if it got one, was already reclaimed above as an
            // orphan — unless it was recorded after that pass read the list, so
            // this is belt and braces rather than duplication.
            if let (Some(handle), Some(adapter)) =
                (handle, self.adapters.get(&record.session.kernel))
            {
                let _ = adapter.terminate(&handle).await;
            }
            reclaimed.abandoned_sessions += 1;
        }

        if reclaimed != Reclaimed::default() {
            tracing::info!(
                orphaned_runtimes = reclaimed.orphaned_runtimes,
                abandoned_sessions = reclaimed.abandoned_sessions,
                "reconciliation reclaimed resources"
            );
        }
        reclaimed
    }
}
