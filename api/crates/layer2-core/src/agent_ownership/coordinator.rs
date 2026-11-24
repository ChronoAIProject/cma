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

const RECONCILIATION_OWNER_READ_BUDGET: Duration = Duration::from_millis(200);

#[path = "coordinator_activation.rs"]
mod activation;
#[path = "coordinator_batch_renewal.rs"]
mod batch;
#[path = "coordinator_heartbeat.rs"]
mod heartbeat;
#[cfg(feature = "qualification-hooks")]
#[path = "coordinator_qualification.rs"]
mod qualification;
#[cfg(feature = "qualification-hooks")]
pub use qualification::CapturedOwner;
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
    #[cfg(feature = "qualification-hooks")]
    qualification_renewal: std::sync::OnceLock<Arc<dyn crate::qualification::RenewalHook>>,
    acquisition: Mutex<()>,
    cancelled_cursor: AtomicUsize,
    member_deadline: std::sync::Mutex<Instant>,
    skipped_heartbeats: AtomicU32,
    draining: AtomicBool,
    failed: watch::Sender<bool>,
    batch_maintenance: Option<Mutex<()>>,
    pub(super) cleanup_pacing: super::cleanup_pacing::CleanupPacing,
    /// The member record held back until activation registers it.
    registration: std::sync::Mutex<Option<Member>>,
    registered: AtomicBool,
}

impl OwnershipCoordinator {
    pub async fn register(
        store: Arc<dyn OwnershipStore>,
        member: Member,
    ) -> Result<Arc<Self>, OwnershipError> {
        Self::register_with_cleanup_pacing(store, member, false).await
    }

    /// Register immediately, for callers that can already serve.
    pub async fn register_with_cleanup_pacing(
        store: Arc<dyn OwnershipStore>,
        member: Member,
        enabled: bool,
    ) -> Result<Arc<Self>, OwnershipError> {
        let coordinator = Self::prepare_with_cleanup_pacing(store, member, enabled);
        coordinator.activate().await?;
        Ok(coordinator)
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
            // Drain joins this acquisition before its snapshot. Keep a lease
            // that committed meanwhile so drain releases it promptly instead of
            // leaving it to expire.
            if self.is_draining() {
                owned.insert(id.clone(), authority);
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

    /// A live foreign lease can defer one background reconciliation visit.
    /// The store checks expiry. No lease observation is retained or grants authority.
    pub async fn observe_foreign_reconciliation_owner(&self, id: &AgentId) -> bool {
        if !self.can_observe_reconciliation_owner(id).await {
            return false;
        }
        let Ok(Ok(Some(lease))) =
            tokio::time::timeout(RECONCILIATION_OWNER_READ_BUDGET, self.store.owner(id)).await
        else {
            return false;
        };
        let token = lease.token;
        token.agent_id == *id
            && token.protocol == super::PROTOCOL_VERSION
            && token.epoch > 0
            && !token.instance_id.is_empty()
            && token.instance_id != self.instance_id
            && self.can_observe_reconciliation_owner(id).await
    }

    async fn can_observe_reconciliation_owner(&self, id: &AgentId) -> bool {
        // Let ordinary acquisition handle all locally tracked authorities,
        // including revoked authorities and unresolved cancelled receivers.
        self.is_ready()
            && self.skipped_heartbeats.load(Ordering::Acquire) == 0
            && !self.owned.lock().await.contains_key(id)
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
        // Join an in-flight acquisition or activation first, so the snapshot
        // includes whatever committed before admission closed.
        let _acquisition = self.acquisition.lock().await;
        let owned: Vec<_> = self.owned.lock().await.values().cloned().collect();
        for authority in &owned {
            authority.revoke();
        }
        // Failure to advertise drain never restores local authority. In that
        // case peers can still take over after the expiring durable lease.
        // A member that never registered has nothing to advertise.
        if self.is_registered() {
            let _ = bounded(self.store.heartbeat(&self.instance_id, true)).await;
        }
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

    /// Drop the local custody of a retiring authority whose handoff could not
    /// confirm revocation, once the store shows its exact token no longer
    /// holds a live lease: it expired, was released or was replaced. An
    /// expired owner token cannot be renewed, so this is final for the token
    /// and equals the takeover a restart would allow. It never releases,
    /// renews or changes another epoch, and returns false while the token
    /// still holds the lease.
    pub async fn forget_unconfirmed_retirement(
        &self,
        authority: &Arc<Authority>,
    ) -> Result<bool, OwnershipError> {
        if !authority.retiring.load(Ordering::Acquire) || authority.receiver_cancelled() {
            return Ok(false);
        }
        let mut complete = authority.handoff_complete.lock().await;
        if *complete {
            return Ok(true);
        }
        let owner = bounded(self.store.owner(&authority.token.agent_id)).await?;
        if owner.is_some_and(|lease| lease.token == authority.token) {
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
