//! When bringing a runtime up does not work.
//!
//! Split from the successful path so both stay within the project's file-size
//! limit, on a division that is real: that file is what happens when a session
//! starts, and this is what happens when it does not.
//!
//! Nobody is waiting to be told. Provisioning is detached from the request
//! that asked for it, so a failure that is merely logged leaves a session
//! sitting in `provisioning` forever — worse than the synchronous version,
//! where at least the caller saw the error. Every path through this module
//! therefore ends in a recorded state.

use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{AdapterError, KernelAdapter, RuntimeHandle};

use crate::event::EventKind;
use crate::ids::SessionId;
use crate::provision::PROVISION_ATTEMPTS;
use crate::service::SessionService;
use crate::session::{RecoveryState, Session, SessionSpec, SessionStatus, StopReason};
use crate::terminal::TerminalGeometry;

/// Why a bring-up attempt failed, and whether another one could go differently.
///
/// Most provisioning failures are nobody's decision and usually pass, which is
/// what the retries are for. Some are answers: a runtime asked for something
/// its kernel does not do will be told the same thing on the fourth attempt as
/// on the first, and making its owner wait out the backoff to hear it turns a
/// clear refusal into what looks like a slow, flaky start.
pub(crate) struct StartFailure {
    /// What to record against the session, written for whoever has to act.
    pub(crate) reason: String,
    /// Whether trying again could produce a different outcome.
    pub(crate) retryable: bool,
    /// The ability that was missing, when the failure was a refusal.
    ///
    /// Kept beside the sentence rather than only inside it. The refusal is
    /// arrived at typed, and a caller downstream can act on a name where it
    /// can do nothing with a sentence that happens to contain one.
    pub(crate) capability: Option<String>,
}

impl StartFailure {
    /// A failure that another attempt might get past.
    pub(crate) fn transient(reason: String) -> Self {
        Self {
            reason,
            retryable: true,
            capability: None,
        }
    }

    /// A failure that every attempt would meet identically.
    pub(crate) fn permanent(reason: String) -> Self {
        Self {
            reason,
            retryable: false,
            capability: None,
        }
    }

    /// Name the ability this failure was about.
    pub(crate) fn lacking(mut self, capability: impl Into<String>) -> Self {
        self.capability = Some(capability.into());
        self
    }

    /// Classify what the provider said about bringing a runtime up.
    ///
    /// An absent ability is the one answer retrying cannot change: the adapter
    /// distinguishes it from a failure precisely so a caller need not read
    /// error strings to tell "cannot" from "did not".
    ///
    /// **Nothing is named here, deliberately.** `AdapterError::Unsupported`
    /// carries a sentence written for a person — "this kernel does not render
    /// a terminal", "this runtime's image does not carry ..." — and a
    /// sentence is not a name. Putting one in `capability` would promise a
    /// caller something it could act on and hand it prose to parse, which is
    /// worse than admitting there is no name to give.
    pub(crate) fn from_provisioning(error: AdapterError) -> Self {
        let reason = format!("could not provision a runtime: {error}");
        match error {
            AdapterError::Unsupported(_) => Self::permanent(reason),
            AdapterError::Unavailable(_) | AdapterError::Rejected(_) => Self::transient(reason),
        }
    }
}

impl SessionService {
    /// Keep trying to bring a runtime up until one does, or the attempts run out.
    ///
    /// Each attempt starts clean. A previous attempt may have created a runtime
    /// before failing — an attach that timed out leaves one running — and
    /// retrying without tearing that down would leak one sandbox per attempt.
    pub(crate) async fn start_with_retries(
        self: &Arc<Self>,
        session: &Session,
        spec: &SessionSpec,
        adapter: &Arc<dyn KernelAdapter>,
        geometry: TerminalGeometry,
        restore: Option<&Vec<u8>>,
    ) -> Result<(), StartFailure> {
        let mut backoff = Duration::from_millis(
            self.retry_backoff_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        // The last thing that went wrong, kept whole. Reduced to a sentence
        // it would lose the name of whatever was missing, which is the part
        // the owner can act on.
        let mut last = StartFailure::transient(String::new());

        for attempt in 1..=PROVISION_ATTEMPTS {
            match self
                .start_runtime(session, spec, adapter, geometry, restore)
                .await
            {
                Ok(()) => {
                    if attempt > 1 {
                        tracing::info!(
                            session_id = %session.id,
                            attempt,
                            "runtime started after retrying"
                        );
                    }
                    return Ok(());
                }
                Err(failure) if !failure.retryable => {
                    // The answer will not change, so waiting out the backoff
                    // only delays telling the owner something already known.
                    tracing::warn!(
                        session_id = %session.id,
                        reason = %failure.reason,
                        "this session cannot be started at all; not retrying"
                    );
                    self.discard_runtime(&session.id, adapter).await;
                    return Err(failure);
                }
                Err(failure) => {
                    tracing::warn!(
                        session_id = %session.id,
                        attempt,
                        of = PROVISION_ATTEMPTS,
                        reason = %failure.reason,
                        "could not start the runtime; trying again"
                    );
                    self.discard_runtime(&session.id, adapter).await;
                    last = failure;
                }
            }
            if attempt < PROVISION_ATTEMPTS {
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }
        }

        let exhausted = format!(
            "could not start a runtime after {PROVISION_ATTEMPTS} attempts. Last error: {}",
            last.reason
        );
        Err(StartFailure {
            reason: exhausted,
            ..last
        })
    }

    /// Tear down whatever a failed attempt left behind.
    ///
    /// Without this each retry strands the runtime the previous one created,
    /// which is how a session that eventually succeeds still costs several
    /// sandboxes.
    async fn discard_runtime(&self, id: &SessionId, adapter: &Arc<dyn KernelAdapter>) {
        let Ok(Some(mut record)) = self.store.get(id).await else {
            return;
        };
        let Some(handle) = record.runtime_id.take().map(RuntimeHandle) else {
            return;
        };
        if let Err(e) = adapter.terminate(&handle).await {
            tracing::warn!(session_id = %id, error = %e, "could not discard a failed runtime");
        }
        if let Err(e) = self.store.upsert(&record).await {
            tracing::error!(session_id = %id, error = %e, "cannot clear the discarded runtime");
        }
        self.entries.write().await.remove(id);
    }

    /// Give up on a session, saying why, and take any runtime down with it.
    ///
    /// The state is written even though nobody is waiting for it. That is the
    /// whole point: the caller left long ago, and a session that fails silently
    /// is indistinguishable from one still starting.
    pub(crate) async fn abandon(
        &self,
        id: &SessionId,
        adapter: &Arc<dyn KernelAdapter>,
        failure: &StartFailure,
    ) {
        let reason = failure.reason.as_str();
        tracing::error!(
            session_id = %id,
            reason,
            capability = failure.capability.as_deref().unwrap_or("-"),
            "session could not be started"
        );

        let runtime = match self.store.get(id).await {
            Ok(Some(mut record)) => {
                let handle = record.runtime_id.take().map(RuntimeHandle);
                record.session.status = SessionStatus::Terminated;
                record.session.stop_reason = Some(StopReason::Error);
                record.session.recovery_state = RecoveryState::Retired;
                let seq = self.sequences.take(id, record.session.last_seq, 1);
                record.session.last_seq = seq;
                let session_id = record.session.id.clone();
                if let Err(e) = self.store.upsert(&record).await {
                    tracing::error!(session_id = %id, error = %e, "cannot record the failure");
                }
                if let Err(e) = self
                    .append_durably(
                        id,
                        vec![crate::event::Event {
                            session_id,
                            seq,
                            kind: EventKind::StatusChanged {
                                status: SessionStatus::Terminated,
                                stop_reason: Some(StopReason::Error),
                                // The reason travels with the event so the person
                                // watching learns what the log already knew.
                                detail: Some(reason.to_string()),
                                // Named where there is a name. This is the
                                // whole point of carrying it this far: a
                                // client can say *which* ability the session
                                // lacked rather than "something went wrong".
                                capability: failure.capability.clone(),
                            },
                        }],
                    )
                    .await
                {
                    tracing::error!(session_id = %id, error = %e, "cannot record the failure event");
                }
                handle
            }
            _ => None,
        };

        self.entries.write().await.remove(id);

        // A runtime may exist even though the session failed — provisioning can
        // succeed and attaching still fail. Leaving it running would be the
        // leak this design exists to prevent.
        if let Some(handle) = runtime {
            if let Err(e) = adapter.terminate(&handle).await {
                tracing::warn!(session_id = %id, error = %e, "could not tear down the runtime");
            }
        }
    }
}
