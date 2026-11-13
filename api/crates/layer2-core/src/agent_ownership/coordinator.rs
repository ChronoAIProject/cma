use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
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

/// One process incarnation. Loss of renewal permanently revokes this incarnation;
/// the composition stops it and a restart registers a fresh random identity.
pub struct OwnershipCoordinator {
    pub instance_id: String,
    pub store: Arc<dyn OwnershipStore>,
    owned: Mutex<BTreeMap<AgentId, Arc<Authority>>>,
    member_deadline: std::sync::Mutex<Instant>,
    draining: AtomicBool,
    failed: watch::Sender<bool>,
}

impl OwnershipCoordinator {
    pub async fn register(
        store: Arc<dyn OwnershipStore>,
        member: Member,
    ) -> Result<Arc<Self>, OwnershipError> {
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
            store,
            owned: Mutex::new(BTreeMap::new()),
            member_deadline: std::sync::Mutex::new(deadline),
            draining: AtomicBool::new(false),
            failed: watch::channel(false).0,
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

    pub async fn acquire(&self, id: &AgentId) -> Result<Option<Arc<Authority>>, OwnershipError> {
        if !self.is_ready() {
            return Err(OwnershipError::Stale);
        }
        // Serialize local acquisitions, including uncertain retries. Only the
        // distributed store's CAS can grant authority between different processes.
        let mut owned = self.owned.lock().await;
        if !self.is_ready() {
            return Err(OwnershipError::Stale);
        }
        if let Some(authority) = owned.get(id) {
            return if authority.is_current() {
                Ok(Some(authority.clone()))
            } else {
                Err(OwnershipError::Stale)
            };
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
        let authority = Authority::with_store(
            lease.token,
            deadline(started, membership.now_ms, lease.expires_at_ms)?,
            self.store.clone(),
        );
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
    pub async fn maintain(&self) -> Result<Vec<Arc<Authority>>, OwnershipError> {
        let result = self.maintain_inner().await;
        if result.is_err() {
            self.fail().await;
        }
        result
    }

    async fn maintain_inner(&self) -> Result<Vec<Arc<Authority>>, OwnershipError> {
        if !self.is_ready() {
            return Err(OwnershipError::Stale);
        }
        let started = Instant::now();
        let membership = bounded(self.store.heartbeat(&self.instance_id, false)).await?;
        let member = membership
            .members
            .iter()
            .find(|member| member.instance_id == self.instance_id)
            .ok_or(OwnershipError::Stale)?;
        let member_deadline = deadline(started, membership.now_ms, member.expires_at_ms)?;
        {
            let mut current = self.member_deadline.lock().unwrap();
            if *current <= Instant::now() {
                return Err(OwnershipError::Stale);
            }
            *current = member_deadline;
        }
        let owned: Vec<_> = self.owned.lock().await.values().cloned().collect();
        stream::iter(owned.iter().cloned().map(|authority| async move {
            let lease = bounded(self.store.renew(&authority.token)).await?;
            require_same_token(&authority.token, &lease)?;
            authority.renew_until(deadline(started, membership.now_ms, lease.expires_at_ms)?)
        }))
        .buffer_unordered(4)
        .try_collect::<Vec<_>>()
        .await?;
        let mut handoffs = Vec::new();
        for authority in owned {
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

    pub async fn begin_drain(&self) -> Vec<Arc<Authority>> {
        self.draining.store(true, Ordering::Release);
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
        Ok(())
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
