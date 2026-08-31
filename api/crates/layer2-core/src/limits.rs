//! Live-session limits, applied only while an owner is asking for a runtime.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Weak};

use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::ids::SubjectId;
use crate::service::{store_failed, SessionService};
use crate::store::SessionStore;

/// What the deployment currently permits this caller to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionAllowance {
    /// Absent when the deployment has set no live-session limit.
    pub limit: Option<u32>,
    /// Sessions this subject has recorded as wanting a runtime.
    pub live: u32,
    /// Whether the caller is explicitly exempt from the limit.
    pub exempt: bool,
}

/// One in-process mutex per owner who currently has a limit check in flight.
///
/// This narrows the race; it does not close it. The deployment currently pins
/// one control-plane replica with a replacement strategy, so an in-process
/// guard is sufficient today. The control plane is designed to scale out, and
/// then the accepted overshoot is bounded by the number of replicas making
/// concurrent requests. This is a policy limit, not a security boundary:
/// overshooting admits work an owner asked for, while undershooting refuses
/// work they are entitled to. A hard cross-process limit needs a conditional
/// store write.
#[derive(Default)]
pub(crate) struct OwnerGuards {
    held: Mutex<HashMap<SubjectId, Weak<Mutex<()>>>>,
}

impl OwnerGuards {
    pub(crate) async fn lock(&self, owner: &SubjectId) -> OwnedMutexGuard<()> {
        let lock = {
            let mut held = self.held.lock().await;
            held.retain(|_, lock| lock.strong_count() > 0);
            match held.get(owner).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    held.insert(owner.clone(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }

    /// Take an owner's local mutation guard only when it is immediately free.
    /// Opportunistic maintenance uses this to avoid waiting while it already
    /// holds per-session admission fences.
    pub(crate) async fn try_lock(&self, owner: &SubjectId) -> Option<OwnedMutexGuard<()>> {
        let lock = {
            let mut held = self.held.lock().await;
            held.retain(|_, lock| lock.strong_count() > 0);
            match held.get(owner).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    held.insert(owner.clone(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.try_lock_owned().ok()
    }
}

struct BoundGuard {
    owner: SubjectId,
    limit: u32,
    _held: OwnedMutexGuard<()>,
}

/// Keeps a successful limit check serialized through the record write.
pub(crate) struct SessionLimitGuard(Option<BoundGuard>);

impl SessionLimitGuard {
    /// Observe the authoritative count after the write, then release the guard.
    pub(crate) async fn record_written(self, store: &dyn SessionStore) {
        let Some(bound) = self.0 else {
            return;
        };
        match store.count_live_for(&bound.owner).await {
            Ok(live) if live > bound.limit => tracing::warn!(
                subject = %bound.owner,
                live,
                limit = bound.limit,
                "live-session limit overshot across control-plane processes"
            ),
            Ok(_) => {}
            Err(error) => tracing::warn!(
                subject = %bound.owner,
                error = %error,
                "could not verify the live-session count after writing a session"
            ),
        }
    }
}

impl SessionService {
    /// Fail unless this caller may hold one more live session.
    ///
    /// Requiring a `Caller` is the structural boundary: reconciliation,
    /// recovery and runtime replacement have no caller and cannot reach this
    /// policy. They restore work an owner already established rather than
    /// asking for another session.
    ///
    /// A successful bound check returns its owner guard. The asking path keeps
    /// that value alive through the store write, so two local requests cannot
    /// both count the same free slot.
    pub(crate) async fn refuse_at_session_limit(
        &self,
        caller: &Caller,
        limit: Option<NonZeroU32>,
    ) -> Result<SessionLimitGuard, CoreError> {
        if caller.authorization.allows(Capability::ExceedSessionLimit) {
            return Ok(SessionLimitGuard(None));
        }
        let Some(limit) = limit else {
            return Ok(SessionLimitGuard(None));
        };

        let held = self.session_limit_guards.lock(&caller.subject).await;
        let live = self
            .store
            .count_live_for(&caller.subject)
            .await
            .map_err(store_failed)?;
        if live >= limit.get() {
            return Err(CoreError::SessionLimitReached {
                limit: limit.get(),
                live,
            });
        }
        Ok(SessionLimitGuard(Some(BoundGuard {
            owner: caller.subject.clone(),
            limit: limit.get(),
            _held: held,
        })))
    }
}
