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

use crate::coordination::SessionLease;
use crate::error::CoreError;
use crate::event::EventKind;
use crate::ids::SessionId;
use crate::provision::{RuntimeStartContext, PROVISION_ATTEMPTS};
use crate::service::{store_failed, SessionService};
use crate::session::{RecoveryState, Session, SessionSpec, SessionStatus, StopReason};
use crate::store::{SessionRecord, StoreError};

const AT_CAPACITY_REASON: &str = "This deployment has no room for a session this size right now. Nothing is wrong with your session or your request — stop a session, or ask for a smaller one, and try again.";

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
    /// Which runtime identity this failure is allowed to clean up.
    cleanup: StartFailureCleanup,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StartFailureCleanup {
    /// The failed attempt still agrees with the current durable row.
    DurableRecord,
    /// Cancellation or a concurrent successor made row-derived cleanup unsafe.
    UnverifiedAttempt,
}

impl StartFailure {
    /// A failure that another attempt might get past.
    pub(crate) fn transient(reason: String) -> Self {
        Self {
            reason,
            retryable: true,
            capability: None,
            cleanup: StartFailureCleanup::DurableRecord,
        }
    }

    /// A failure that every attempt would meet identically.
    pub(crate) fn permanent(reason: String) -> Self {
        Self {
            reason,
            retryable: false,
            capability: None,
            cleanup: StartFailureCleanup::DurableRecord,
        }
    }

    /// A transient failure whose exact runtime authority cannot be re-established.
    ///
    /// Retrying may strand an existing candidate and create another; deriving
    /// cleanup from the current row could instead delete a successor.
    pub(crate) fn unverified_attempt(reason: String) -> Self {
        Self {
            reason,
            retryable: true,
            capability: None,
            cleanup: StartFailureCleanup::UnverifiedAttempt,
        }
    }

    fn may_retry_start(&self) -> bool {
        self.retryable && self.cleanup == StartFailureCleanup::DurableRecord
    }

    fn may_clean_up_durable_record(&self) -> bool {
        self.cleanup == StartFailureCleanup::DurableRecord
    }

    /// Name the ability this failure was about.
    pub(crate) fn lacking(mut self, capability: impl Into<String>) -> Self {
        self.capability = Some(capability.into());
        self
    }

    /// Classify what the provider said about bringing a runtime up.
    ///
    /// Two answers cannot change inside a retry loop: an absent ability and an
    /// absent space. Four attempts over minutes do not add either one, so both
    /// are distinguished from transient failures before a caller waits for a
    /// backoff that cannot help.
    ///
    /// **Nothing is named here, deliberately.** `AdapterError::Unsupported`
    /// carries a sentence written for a person — "this kernel does not render
    /// a terminal", "this runtime's image does not carry ..." — and a
    /// sentence is not a name. Putting one in `capability` would promise a
    /// caller something it could act on and hand it prose to parse, which is
    /// worse than admitting there is no name to give.
    pub(crate) fn from_provisioning(error: AdapterError) -> Self {
        match error {
            AdapterError::AtCapacity(_) => Self::permanent(AT_CAPACITY_REASON.to_string()),
            AdapterError::Unsupported(_) => {
                Self::permanent(format!("could not provision a runtime: {error}"))
            }
            AdapterError::Unavailable(_) | AdapterError::Rejected(_) => {
                Self::transient(format!("could not provision a runtime: {error}"))
            }
        }
    }
}

/// What becomes of a session whose runtime could not be brought up.
///
/// Stated by whoever asked for the bring-up rather than inferred here, because
/// the two cases are not distinguishable from inside and getting it wrong
/// costs somebody their work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Abandonment {
    /// Nothing existed yet. Terminating is right and says so.
    ///
    /// A session that never started has no history, no place in a list and
    /// nothing to carry; leaving it half-alive would be worse than ending it.
    Retire,
    /// The session already existed, and must survive this.
    ///
    /// A rotation or a revival: it has an identifier, a history, a place in its
    /// owner's list, and — for a rotation — a snapshot taken precisely so it
    /// could be carried. `CLAUDE.md` §5 makes losing that as severe as losing
    /// the data, and a rotation is something a *deployment* caused rather than
    /// anything its owner asked for. An ordinary revival goes back to resting;
    /// a descriptor-backed handover remains rotating so reconciliation retries
    /// it without waiting for its owner.
    Keep,
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
        context: RuntimeStartContext<'_>,
        lease: &mut Option<SessionLease>,
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
                .start_runtime(session, spec, adapter, context, lease)
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
                Err(failure) if !failure.may_retry_start() => {
                    // A permanent answer cannot improve with backoff. An
                    // unverified attempt cannot be retried either: the next
                    // attempt could strand a candidate, while cleanup
                    // derived from the row could delete a concurrent successor.
                    tracing::warn!(
                        session_id = %session.id,
                        reason = %failure.reason,
                        "this runtime start cannot be retried safely"
                    );
                    if lease.is_none() && failure.may_clean_up_durable_record() {
                        if let Err(error) = self.discard_runtime(&session.id, adapter).await {
                            return Err(StartFailure::unverified_attempt(format!(
                                "the failed runtime could not be discarded safely: {error}"
                            )));
                        }
                    }
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
                    if lease.is_none() {
                        if let Err(error) = self.discard_runtime(&session.id, adapter).await {
                            return Err(StartFailure::unverified_attempt(format!(
                                "the failed runtime could not be discarded safely: {error}"
                            )));
                        }
                    }
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
    pub(crate) async fn discard_runtime(
        &self,
        id: &SessionId,
        adapter: &Arc<dyn KernelAdapter>,
    ) -> Result<(), CoreError> {
        let Some(record) = self.store.get(id).await.map_err(store_failed)? else {
            return Ok(());
        };
        if record.runtime_id.is_none() {
            return Ok(());
        }
        self.teardown_compatibility_runtime(adapter, &record, "failed runtime cleanup")
            .await?;
        self.entries.write().await.remove(id);
        Ok(())
    }

    /// Delete one compatibility runtime before conditionally clearing its record.
    ///
    /// Keeping the handle durable through provider success makes a transient
    /// teardown failure retryable. A successful delete followed by a failed
    /// clear is safe too: provider deletion is idempotent, and the semantic
    /// clear refuses a changed lifecycle, claim, or successor runtime.
    pub(crate) async fn teardown_compatibility_runtime(
        &self,
        adapter: &Arc<dyn KernelAdapter>,
        claimed: &SessionRecord,
        operation: &str,
    ) -> Result<SessionRecord, CoreError> {
        let handle = claimed
            .runtime_id
            .clone()
            .map(RuntimeHandle)
            .ok_or_else(|| {
                CoreError::RuntimeUnavailable(format!(
                    "{operation} requires one durable compatibility runtime"
                ))
            })?;
        self.require_compatibility_provider_mutation(&claimed.session.id, operation)
            .await?;
        adapter.terminate(&handle).await.map_err(|error| {
            CoreError::RuntimeUnavailable(format!(
                "{operation} could not tear down runtime {}: {error}",
                handle.0
            ))
        })?;

        let mut cleared = claimed.clone();
        cleared.runtime_id = None;
        self.clear_compatibility_runtime_durably(&mut cleared, claimed)
            .await
            .map_err(store_failed)?;
        Ok(cleared)
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
        outcome: Abandonment,
        rotation_claim: Option<&str>,
    ) -> Result<Option<SessionRecord>, CoreError> {
        let reason = failure.reason.as_str();
        if !failure.may_clean_up_durable_record() {
            tracing::error!(
                session_id = %id,
                reason,
                "runtime cleanup authority is unverified; leaving durable authority unchanged"
            );
            return self.store.get(id).await.map_err(store_failed);
        }
        // A session that already existed survives this. Its runtime could not
        // be replaced, which is a fact about a provider at a moment, and
        // retiring it would discard an identifier, a history, a place in its
        // owner's list and the snapshot taken so it could be carried —
        // because something was busy. §5 makes that as severe as losing the
        // data, and a rotation is something a deployment caused rather than
        // anything its owner asked for.
        //
        // Keeping only makes sense while another attempt could go differently.
        // A refusal is an answer — a kernel that no longer offers what this
        // session is will not start offering it — so a session kept against
        // one would rest forever, be retried forever by the sweep, and tell
        // its owner it was coming back when it was not. `retryable` is the
        // adapter's own word for which of the two this is, and it is already
        // carried this far.
        let keep = matches!(outcome, Abandonment::Keep) && failure.retryable;

        tracing::error!(
            session_id = %id,
            reason,
            capability = failure.capability.as_deref().unwrap_or("-"),
            keeping = keep,
            "a runtime could not be brought up"
        );
        let committed = match self.store.get(id).await.map_err(store_failed)? {
            Some(mut record) => {
                let descriptor_handover = match (rotation_claim, record.rotation_claim.as_deref()) {
                    (Some(expected), Some(current)) if expected == current => {
                        match record.pending_handover.as_ref() {
                            Some(pending) if pending.claim == expected => true,
                            None => false,
                            Some(_) => {
                                return Err(store_failed(StoreError::SessionAuthorityChanged {
                                    id: id.clone(),
                                }));
                            }
                        }
                    }
                    (None, None) if record.pending_handover.is_none() => false,
                    (None, None) => {
                        // Only an explicit owner stop may leave an unconsumed
                        // descriptor idle without its active claim. A generic
                        // failure path cannot silently convert that recovery
                        // boundary into an ordinary revival.
                        return Err(store_failed(StoreError::SessionAuthorityChanged {
                            id: id.clone(),
                        }));
                    }
                    _ => {
                        return Err(store_failed(StoreError::SessionAuthorityChanged {
                            id: id.clone(),
                        }));
                    }
                };
                let retry_handover = keep && descriptor_handover;
                let (status, stop_reason, recovery) = if retry_handover {
                    // H1 is already gone and this descriptor is the only
                    // authoritative source for H2. Preserve the exact claim
                    // and descriptor, and keep the public lifecycle fenced so
                    // the ordinary reconciliation sweep retries it.
                    (SessionStatus::Rotating, None, record.session.recovery_state)
                } else if keep {
                    // Reclaimed and owner-revivable. No automatic attempt is
                    // in flight, so `Recovering` would overstate its state.
                    record.rotation_claim = None;
                    (
                        SessionStatus::Idle,
                        Some(StopReason::Error),
                        RecoveryState::Idle,
                    )
                } else {
                    record.rotation_claim = None;
                    record.pending_handover = None;
                    (
                        SessionStatus::Terminated,
                        Some(StopReason::Error),
                        RecoveryState::Retired,
                    )
                };
                record.session.status = status;
                record.session.stop_reason = stop_reason;
                record.session.recovery_state = recovery;
                let session_id = record.session.id.clone();
                self.persist_durably(
                    &mut record,
                    vec![crate::event::Event {
                        session_id,
                        seq: 0,
                        kind: EventKind::StatusChanged {
                            status,
                            stop_reason,
                            // The reason travels with the event so the person
                            // watching learns what the log already knew —
                            // and, where the session survives, that it
                            // survived, because a stop reason of `error`
                            // otherwise reads as the end of it.
                            detail: Some(match (keep, retry_handover) {
                                (false, _) => reason.to_string(),
                                (true, true) => format!(
                                    "{reason}. This session was not lost: its stored handover remains fenced and will be retried automatically."
                                ),
                                (true, false) => format!(
                                    "{reason}. This session was not lost: it is resting and can be revived."
                                ),
                            }),
                            // Named where there is a name. This is the
                            // whole point of carrying it this far: a
                            // client can say *which* ability the session
                            // lacked rather than "something went wrong".
                            capability: failure.capability.clone(),
                        },
                        appended_at: String::new(),
                    }],
                )
                .await
                .map_err(store_failed)?;
                Some(record)
            }
            None => None,
        };

        self.entries.write().await.remove(id);

        // A runtime may exist even though the session failed — provisioning can
        // succeed and attaching still fail. Leaving it running would be the
        // leak this design exists to prevent.
        let settled = match committed {
            Some(record) if record.runtime_id.is_some() => self
                .teardown_compatibility_runtime(adapter, &record, "runtime abandonment")
                .await
                .map(Some),
            committed => Ok(committed),
        }?;

        if !keep {
            if let Some(source) = self.credentials.as_ref() {
                if let Err(error) = source.release(id).await {
                    tracing::error!(
                        session_id = %id,
                        %error,
                        "could not retire the abandoned session's credential custody"
                    );
                }
            }
        }
        Ok(settled)
    }
}
