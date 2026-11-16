use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, OnceLock,
};
use std::time::Duration;

use futures_util::{stream, StreamExt, TryStreamExt};
use tokio::sync::{watch, Mutex};
use tokio::time::Instant;

use super::{
    candidates, Authority, Member, OwnershipError, OwnershipLease, OwnershipStore, OwnershipToken,
    STORE_DEADLINE_MS,
};
use crate::AgentId;

#[path = "renewal_containment.rs"]
mod renewal_containment;

/// One process incarnation. Membership loss permanently revokes this incarnation;
/// an unavailable owner renewal can cancel only its still-current Agent.
pub struct OwnershipCoordinator {
    pub instance_id: String,
    pub store: Arc<dyn OwnershipStore>,
    owned: Mutex<BTreeMap<AgentId, Arc<Authority>>>,
    acquisition: Mutex<()>,
    registration: std::sync::Mutex<Option<Member>>,
    registered: AtomicBool,
    admission_readiness: OnceLock<Arc<dyn Fn() -> bool + Send + Sync>>,
    uncertain_acquisitions: AtomicUsize,
    cancelled_cursor: AtomicUsize,
    member_deadline: std::sync::Mutex<Instant>,
    draining: AtomicBool,
    failed: watch::Sender<bool>,
}

mod admission;

impl OwnershipCoordinator {
    pub fn is_ready(&self) -> bool {
        self.registered.load(Ordering::Acquire)
            && !self.draining.load(Ordering::Acquire)
            && !*self.failed.borrow()
            && Instant::now() < *self.member_deadline.lock().unwrap()
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }

    pub fn failure_signal(&self) -> watch::Receiver<bool> {
        self.failed.subscribe()
    }

    pub async fn acquire(&self, id: &AgentId) -> Result<Option<Arc<Authority>>, OwnershipError> {
        if !self.can_acquire() {
            return Err(OwnershipError::Stale);
        }
        // Serialize local acquisitions, including uncertain retries, without
        // holding the owned-map lock across IO needed by renewal and drain.
        // Only the store CAS grants authority between processes.
        let _acquisition = self.acquisition.lock().await;
        if !self.can_acquire() {
            return Err(OwnershipError::Stale);
        }
        let cached = {
            let owned = self.owned.lock().await;
            if !self.can_acquire() {
                return Err(OwnershipError::Stale);
            }
            owned.get(id).cloned()
        };
        let cancelled = match cached {
            Some(authority) if authority.is_current() => return Ok(Some(authority)),
            Some(authority) if authority.receiver_cancelled() => Some(authority),
            Some(_) => return Err(OwnershipError::Stale),
            None => None,
        };
        let started = Instant::now();
        let membership = bounded(self.store.membership()).await?;
        if !self.can_acquire() {
            return Err(OwnershipError::Stale);
        }
        let acquired = bounded(self.store.acquire(id, &self.instance_id)).await;
        if matches!(&acquired, Err(OwnershipError::Unavailable(_))) {
            // No exact token is available to release safely after an ambiguous
            // write. Preserve that uncertainty in the terminal drain result.
            self.uncertain_acquisitions.fetch_add(1, Ordering::Release);
        }
        let Some(lease) = acquired? else {
            return Ok(None);
        };
        if lease.token.instance_id != self.instance_id || lease.token.agent_id != *id {
            return Err(OwnershipError::Invalid(
                "acquisition returned another owner's token".into(),
            ));
        }
        // An uncertain late renewal may leave the cancelled exact token live.
        // A new caller can receive authority only for a strictly newer epoch.
        if cancelled
            .as_ref()
            .is_some_and(|old| lease.token.epoch <= old.token.epoch)
        {
            return Ok(None);
        }
        let authority = Authority::with_store(
            lease.token,
            deadline(started, membership.now_ms, lease.expires_at_ms)?,
            self.store.clone(),
        );
        let mut owned = self.owned.lock().await;
        if !self.can_acquire() {
            authority.revoke();
            // Keep late committed custody visible to the drain that joins
            // this acquisition. It never became execution authority.
            owned.insert(id.clone(), authority);
            drop(owned);
            // A planned drain already joins this acquisition and owns cleanup.
            // Keep HTTP alive for preStop while that exact custody is drained.
            // Generic readiness loss or an expired member still fails closed.
            let member_expired = Instant::now() >= *self.member_deadline.lock().unwrap();
            if !self.is_draining() || member_expired {
                self.fail().await;
            }
            return Err(OwnershipError::Stale);
        }
        owned.insert(id.clone(), authority.clone());
        Ok(Some(authority))
    }

    pub async fn owned(&self) -> Vec<Arc<Authority>> {
        self.owned.lock().await.values().cloned().collect()
    }

    pub async fn local(&self, id: &AgentId) -> Option<Arc<Authority>> {
        if !self.is_ready() {
            return None;
        }
        self.owned
            .lock()
            .await
            .get(id)
            .filter(|authority| authority.is_current())
            .cloned()
    }

    /// A successful renewal never revives an authority already cancelled locally.
    /// Returns cooperative handoff candidates; callers may defer busy Agents.
    pub async fn maintain(&self) -> Result<Vec<Arc<Authority>>, OwnershipError> {
        let result = self.maintain_inner().await;
        if result.is_err() {
            self.fail().await;
        }
        result
    }

    async fn maintain_inner(&self) -> Result<Vec<Arc<Authority>>, OwnershipError> {
        if self.is_starting() {
            return Ok(Vec::new());
        }
        if !self.is_ready() {
            return Err(self.maintenance_error("member_readiness", None, OwnershipError::Stale));
        }
        let started = Instant::now();
        let membership = bounded(self.store.heartbeat(&self.instance_id, false))
            .await
            .map_err(|error| self.maintenance_error("member_heartbeat", None, error))?;
        let member = membership
            .members
            .iter()
            .find(|member| member.instance_id == self.instance_id)
            .ok_or_else(|| self.maintenance_error("member_lookup", None, OwnershipError::Stale))?;
        let member_deadline = deadline(started, membership.now_ms, member.expires_at_ms)
            .map_err(|error| self.maintenance_error("member_store_deadline", None, error))?;
        {
            let mut current = self.member_deadline.lock().unwrap();
            if *current <= Instant::now() {
                return Err(self.maintenance_error(
                    "member_local_deadline",
                    None,
                    OwnershipError::Stale,
                ));
            }
            *current = member_deadline;
        }
        let owned: Vec<_> = self.owned.lock().await.values().cloned().collect();
        stream::iter(owned.iter().cloned().map(|authority| async move {
            // A handoff can complete while a captured renewal is in flight.
            // Its result cannot revive retired authority or fail healthy peers.
            if authority.retiring.load(Ordering::Acquire) || authority.receiver_cancelled() {
                return Ok(());
            }
            let result = async {
                let Some(lease) = self.renew_owner_or_cancel(&authority).await? else {
                    return Ok(());
                };
                require_same_token(&authority.token, &lease).map_err(|error| {
                    self.maintenance_error("owner_same_token", Some(&authority), error)
                })?;
                let until =
                    deadline(started, membership.now_ms, lease.expires_at_ms).map_err(|error| {
                        self.maintenance_error("owner_store_deadline", Some(&authority), error)
                    })?;
                authority.renew_until(until).map_err(|error| {
                    self.maintenance_error("owner_local_renew", Some(&authority), error)
                })
            }
            .await;
            if authority.retiring.load(Ordering::Acquire) || authority.receiver_cancelled() {
                Ok(())
            } else {
                result
            }
        }))
        .buffer_unordered(4)
        .try_collect::<Vec<_>>()
        .await?;
        // Renewal IO may outlive the membership proof captured above. A local
        // cancellation never permits this process to serve past that deadline.
        if !self.is_ready() {
            return Err(self.maintenance_error(
                "member_local_deadline",
                None,
                OwnershipError::Stale,
            ));
        }
        let mut handoffs = Vec::new();
        for authority in owned {
            if handoffs.len() == 4 {
                break;
            }
            if authority.receiver_cancelled() {
                continue;
            }
            if authority.retiring.load(Ordering::Acquire) {
                continue;
            }
            if candidates(
                &authority.token.agent_id,
                &membership,
                Some(&self.instance_id),
            )?
            .first()
            .is_some_and(|preferred| preferred.instance_id != self.instance_id)
            {
                handoffs.push(authority);
            }
            if handoffs.len() == 4 {
                break;
            }
        }
        Ok(handoffs)
    }

    /// Select cleanup at consumption time: the renewal watch may discard
    /// intermediate candidate sets while receiver IO is blocked.
    pub async fn cancelled_receivers(&self) -> Vec<Arc<Authority>> {
        let owned = self.owned.lock().await;
        let cancelled: Vec<_> = owned.values().filter(|a| a.receiver_cancelled()).collect();
        if cancelled.is_empty() {
            return Vec::new();
        }
        let start = self.cancelled_cursor.fetch_add(4, Ordering::Relaxed) % cancelled.len();
        (0..cancelled.len().min(4))
            .map(|index| cancelled[(start + index) % cancelled.len()].clone())
            .collect()
    }

    fn maintenance_error(
        &self,
        stage: &'static str,
        authority: Option<&Authority>,
        error: OwnershipError,
    ) -> OwnershipError {
        use sha2::{Digest, Sha256};
        let instance_sha256 = format!("{:x}", Sha256::digest(self.instance_id.as_bytes()));
        let agent_sha256 = authority
            .map(|a| format!("{:x}", Sha256::digest(a.token.agent_id.as_str().as_bytes())));
        tracing::warn!(stage, %instance_sha256, ?agent_sha256,
            epoch = authority.map(|a| a.token.epoch),
            receiver_cancelled = authority.is_some_and(Authority::receiver_cancelled),
            error = %error, "Ownership maintenance operation failed");
        error
    }

    pub fn stop_admission(&self) {
        self.draining.store(true, Ordering::Release);
    }

    pub async fn begin_drain(&self) -> Vec<Arc<Authority>> {
        self.stop_admission();
        for authority in self.owned.lock().await.values() {
            authority.revoke();
        }
        let _acquisition = self.acquisition.lock().await;
        let owned: Vec<_> = self.owned.lock().await.values().cloned().collect();
        for authority in &owned {
            authority.revoke();
        }
        // Failure to advertise drain never restores local authority. In that
        // case peers can still take over after the expiring durable lease.
        if self.registered.load(Ordering::Acquire) {
            let _ = bounded(self.store.heartbeat(&self.instance_id, true)).await;
        }
        owned
    }

    pub fn uncertain_acquisitions(&self) -> usize {
        self.uncertain_acquisitions.load(Ordering::Acquire)
    }

    pub async fn handoff<F>(
        &self,
        authority: &Arc<Authority>,
        revoke_transport: F,
    ) -> Result<(), OwnershipError>
    where
        F: std::future::Future<Output = Result<(), OwnershipError>>,
    {
        // Serialize exact-token retirement through receiver confirmation and
        // durable release. Cancellation leaves completion false for safe retry.
        let mut complete = authority.handoff_complete.lock().await;
        if *complete {
            return Ok(());
        }
        authority.retiring.store(true, Ordering::Release);
        authority.revoke();
        // Do not shorten the lease if the receiving runtime has not acknowledged
        // revocation: delayed input/ACK packets could otherwise still be in flight.
        bounded(revoke_transport).await?;
        bounded(self.store.release(&authority.token)).await?;
        let mut owned = self.owned.lock().await;
        if owned
            .get(&authority.token.agent_id)
            .is_some_and(|current| Arc::ptr_eq(current, authority))
        {
            owned.remove(&authority.token.agent_id);
        }
        *complete = true;
        Ok(())
    }

    /// Drop only cancelled local custody after a successful durable observation
    /// proves a newer epoch replaced it. Absence may race an uncertain renewal,
    /// so it retains the cancelled token until ACK/release or a new acquisition.
    /// This never releases a lease or changes a replacement epoch.
    pub async fn forget_cancelled(
        &self,
        authority: &Arc<Authority>,
    ) -> Result<bool, OwnershipError> {
        if !authority.receiver_cancelled() {
            return Ok(false);
        }
        let mut complete = authority.handoff_complete.lock().await;
        if *complete {
            return Ok(true);
        }
        let owner = bounded(self.store.owner(&authority.token.agent_id)).await?;
        if owner.is_none_or(|lease| lease.token.epoch <= authority.token.epoch) {
            return Ok(false);
        }
        let mut owned = self.owned.lock().await;
        if owned
            .get(&authority.token.agent_id)
            .is_some_and(|current| Arc::ptr_eq(current, authority))
        {
            owned.remove(&authority.token.agent_id);
        }
        *complete = true;
        Ok(true)
    }

    pub async fn fail(&self) {
        self.failed.send_replace(true);
        for authority in self.owned.lock().await.values() {
            authority.revoke();
        }
    }
}

fn require_same_token(
    token: &OwnershipToken,
    lease: &OwnershipLease,
) -> Result<(), OwnershipError> {
    if token == &lease.token {
        Ok(())
    } else {
        Err(OwnershipError::Stale)
    }
}

fn deadline(started: Instant, now_ms: i64, expires_ms: i64) -> Result<Instant, OwnershipError> {
    let remaining = expires_ms
        .checked_sub(now_ms)
        .filter(|remaining| *remaining > 0)
        .ok_or(OwnershipError::Stale)?;
    let until = started + Duration::from_millis(remaining as u64);
    if until <= Instant::now() {
        Err(OwnershipError::Stale)
    } else {
        Ok(until)
    }
}

pub async fn bounded<T>(
    operation: impl std::future::Future<Output = Result<T, OwnershipError>>,
) -> Result<T, OwnershipError> {
    tokio::time::timeout(Duration::from_millis(STORE_DEADLINE_MS), operation)
        .await
        .map_err(|_| {
            OwnershipError::Unavailable("ownership operation exceeded its deadline".into())
        })?
}
