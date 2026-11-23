use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use futures_util::{stream, StreamExt, TryStreamExt};
use tokio::sync::{watch, Mutex};
use tokio::time::Instant;

use super::{
    candidates, Authority, Member, Membership, OwnershipError, OwnershipLease, OwnershipStore,
    OwnershipToken, STORE_DEADLINE_MS,
};
use crate::AgentId;

#[path = "coordinator_batch_renewal.rs"]
mod batch;
#[path = "coordinator_heartbeat.rs"]
mod heartbeat;
#[path = "coordinator_renewal.rs"]
mod renewal;
#[cfg(test)]
#[path = "coordinator_renewal_tests.rs"]
mod renewal_tests;

/// One process incarnation. Membership or authority invariant loss revokes it;
/// an unavailable renewal cancels only its still-live captured Agent authority,
/// and an unavailable member heartbeat skips one tick while membership holds.
pub struct OwnershipCoordinator {
    pub instance_id: String,
    pub store: Arc<dyn OwnershipStore>,
    owned: Mutex<BTreeMap<AgentId, Arc<Authority>>>,
    acquisition: Mutex<()>,
    cancelled_cursor: AtomicUsize,
    member_deadline: std::sync::Mutex<Instant>,
    skipped_heartbeats: AtomicU32,
    draining: AtomicBool,
    failed: watch::Sender<bool>,
    batch_maintenance: Option<Mutex<()>>,
    pub(super) cleanup_pacing: super::cleanup_pacing::CleanupPacing,
}

struct RenewalHeartbeat {
    started: Instant,
    member_deadline: Instant,
    membership: Membership,
}

impl OwnershipCoordinator {
    pub async fn register(
        store: Arc<dyn OwnershipStore>,
        member: Member,
    ) -> Result<Arc<Self>, OwnershipError> {
        Self::register_with_cleanup_pacing(store, member, false).await
    }

    pub async fn register_with_cleanup_pacing(
        store: Arc<dyn OwnershipStore>,
        member: Member,
        enabled: bool,
    ) -> Result<Arc<Self>, OwnershipError> {
        Self::register_with_options(store, member, enabled, false).await
    }

    pub async fn register_with_options(
        store: Arc<dyn OwnershipStore>,
        member: Member,
        cleanup_pacing: bool,
        batch_renewal: bool,
    ) -> Result<Arc<Self>, OwnershipError> {
        let capacity = member.max_owned_agents as usize;
        let started = Instant::now();
        let instance_id = member.instance_id.clone();
        let membership = bounded(store.register(member)).await?;
        let member = membership
            .members
            .iter()
            .find(|member| member.instance_id == instance_id)
            .ok_or(OwnershipError::Stale)?;
        let deadline = deadline(started, membership.now_ms, member.expires_at_ms)?;
        Ok(Arc::new(Self {
            instance_id,
            batch_maintenance: (batch_renewal && store.supports_batch_renewal())
                .then(Mutex::default),
            store,
            owned: Mutex::new(BTreeMap::new()),
            acquisition: Mutex::new(()),
            cancelled_cursor: AtomicUsize::new(0),
            member_deadline: std::sync::Mutex::new(deadline),
            skipped_heartbeats: AtomicU32::new(0),
            draining: AtomicBool::new(false),
            failed: watch::channel(false).0,
            cleanup_pacing: super::cleanup_pacing::CleanupPacing::new(cleanup_pacing, capacity),
        }))
    }

    pub fn is_ready(&self) -> bool {
        !self.draining.load(Ordering::Acquire)
            && !*self.failed.borrow()
            && Instant::now() < *self.member_deadline.lock().unwrap()
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }

    pub fn failure_signal(&self) -> watch::Receiver<bool> {
        self.failed.subscribe()
    }

    /// Returns existing current authority. A new acquisition is declined with
    /// None, without a store call, while the latest heartbeat tick was skipped.
    pub async fn acquire(&self, id: &AgentId) -> Result<Option<Arc<Authority>>, OwnershipError> {
        if !self.is_ready() {
            return Err(OwnershipError::Stale);
        }
        // Serialize local acquisitions, including uncertain retries, without
        // holding the owned-map lock across IO needed by renewal and drain.
        // Only the store CAS grants authority between processes.
        let _acquisition = self.acquisition.lock().await;
        if !self.is_ready() {
            return Err(OwnershipError::Stale);
        }
        let cancelled = match self.owned.lock().await.get(id).cloned() {
            Some(authority) if authority.is_current() => return Ok(Some(authority)),
            Some(authority) if authority.receiver_cancelled() => Some(authority),
            Some(_) => return Err(OwnershipError::Stale),
            None => None,
        };
        // A skipped heartbeat may begin an outage that ends in failure: adopt
        // nothing new until a heartbeat succeeds. Existing authority stays.
        if self.skipped_heartbeats.load(Ordering::Acquire) > 0 {
            return Ok(None);
        }
        let started = Instant::now();
        let membership = bounded(self.store.membership()).await?;
        let Some(lease) = bounded(self.store.acquire(id, &self.instance_id)).await? else {
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
        if !self.is_ready() {
            authority.revoke();
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
    /// A skipped heartbeat tick renews nothing and returns no candidates.
    pub async fn maintain(&self) -> Result<Vec<Arc<Authority>>, OwnershipError> {
        let _batch_guard = match &self.batch_maintenance {
            Some(lock) => Some(lock.lock().await),
            None => None,
        };
        let result = self.maintain_inner().await;
        if result.is_err() {
            self.fail().await;
        }
        result
    }

    async fn maintain_inner(&self) -> Result<Vec<Arc<Authority>>, OwnershipError> {
        let Some(mut heartbeat) = self.refresh_membership().await? else {
            return Ok(Vec::new());
        };
        let owned: Vec<_> = self.owned.lock().await.values().cloned().collect();
        if self.batch_maintenance.is_some() {
            if !self.renew_in_batches(&owned, &mut heartbeat).await? {
                return Ok(Vec::new());
            }
        } else {
            let renewals = stream::iter(owned.clone())
                .map(|authority| {
                    let started = heartbeat.started;
                    let now_ms = heartbeat.membership.now_ms;
                    async move { self.renew_owned(&authority, started, now_ms).await }
                })
                .buffer_unordered(4)
                .try_collect::<Vec<_>>();
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(heartbeat.member_deadline) => {
                    return Err(self.maintenance_error("member_batch_deadline", None, OwnershipError::Stale));
                }
                result = renewals => { result?; }
            }
        }
        self.handoff_candidates(owned, &heartbeat.membership)
    }

    async fn refresh_membership(&self) -> Result<Option<RenewalHeartbeat>, OwnershipError> {
        if !self.is_ready() {
            return Err(self.maintenance_error("member_readiness", None, OwnershipError::Stale));
        }
        let started = Instant::now();
        let established_deadline = *self.member_deadline.lock().unwrap();
        let heartbeat = tokio::time::timeout(
            Duration::from_millis(STORE_DEADLINE_MS),
            self.store.heartbeat(&self.instance_id, false),
        );
        let heartbeat = tokio::time::timeout_at(established_deadline, heartbeat)
            .await
            .map_err(|_| {
                self.maintenance_error("member_local_deadline", None, OwnershipError::Stale)
            })?;
        // Neither membership nor any authority is extended by a skipped tick.
        let Some(membership) = self.heartbeat_membership(heartbeat)? else {
            return Ok(None);
        };
        membership
            .require_compatible()
            .map_err(|error| self.maintenance_error("member_protocol", None, error))?;
        let member = membership
            .members
            .iter()
            .find(|member| member.instance_id == self.instance_id)
            .ok_or_else(|| self.maintenance_error("member_lookup", None, OwnershipError::Stale))?;
        if member.draining {
            return Err(self.maintenance_error("member_drain", None, OwnershipError::Stale));
        }
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
        Ok(Some(RenewalHeartbeat {
            started,
            member_deadline,
            membership,
        }))
    }

    fn handoff_candidates(
        &self,
        owned: Vec<Arc<Authority>>,
        membership: &Membership,
    ) -> Result<Vec<Arc<Authority>>, OwnershipError> {
        if !self.is_ready() {
            return Err(self.maintenance_error("member_completion", None, OwnershipError::Stale));
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
                membership,
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
        if !self.is_ready() {
            return Err(self.maintenance_error(
                "member_return_deadline",
                None,
                OwnershipError::Stale,
            ));
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
        let reason = renewal::error_reason(&error);
        tracing::error!(stage, scope = "process", reason, %instance_sha256, ?agent_sha256,
            epoch = authority.map(|a| a.token.epoch),
            receiver_cancelled = authority.is_some_and(Authority::receiver_cancelled),
            "Ownership maintenance operation failed");
        error
    }

    pub fn stop_admission(&self) {
        self.draining.store(true, Ordering::Release);
    }

    pub async fn begin_drain(&self) -> Vec<Arc<Authority>> {
        self.stop_admission();
        let owned: Vec<_> = self.owned.lock().await.values().cloned().collect();
        for authority in &owned {
            authority.revoke();
        }
        // Failure to advertise drain never restores local authority. In that
        // case peers can still take over after the expiring durable lease.
        let _ = bounded(self.store.heartbeat(&self.instance_id, true)).await;
        owned
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
        // Do not shorten the lease until every receiving runtime's cleanup
        // obligation is settled. Delayed input/ACK packets may be in flight.
        bounded(revoke_transport).await?;
        // Cooperative cleanup also ends its retry episode. Old completion
        // must not clear observations belonging to a replacement authority.
        if self
            .owned
            .lock()
            .await
            .get(&authority.token.agent_id)
            .is_some_and(|current| Arc::ptr_eq(current, authority))
        {
            self.cleanup_pacing
                .receiver_cleanup_completed(&authority.token.agent_id);
        }
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
        .map_err(|_| deadline_exceeded())?
}

fn deadline_exceeded() -> OwnershipError {
    OwnershipError::Unavailable("ownership operation exceeded its deadline".into())
}
