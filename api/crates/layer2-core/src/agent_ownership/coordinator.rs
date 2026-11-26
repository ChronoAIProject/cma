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
    candidates, Authority, MaintenancePolicy, Member, OwnershipError, OwnershipLease,
    OwnershipStore, OwnershipToken, STORE_DEADLINE_MS,
};
use crate::AgentId;

#[path = "coordinator_reconciliation.rs"]
mod reconciliation;
pub use reconciliation::ForeignReconciliationOwners;
#[path = "coordinator_acquisition.rs"]
mod acquisition;
#[path = "coordinator_activation.rs"]
mod activation;
#[path = "coordinator_batch_renewal.rs"]
mod batch;
#[path = "coordinator_custody.rs"]
mod custody;
#[path = "coordinator_handoff.rs"]
mod handoff;
#[path = "coordinator_heartbeat.rs"]
mod heartbeat;
#[path = "coordinator_legacy_acquire.rs"]
mod legacy_acquire;
#[cfg(feature = "qualification-hooks")]
#[path = "coordinator_qualification.rs"]
mod qualification;
#[cfg(feature = "qualification-hooks")]
pub use qualification::{qualification_owner_digest, CapturedOwner, RenewalWindow};
#[path = "coordinator_renewal.rs"]
mod renewal;
#[cfg(test)]
#[path = "coordinator_renewal_tests.rs"]
mod renewal_tests;
#[path = "coordinator_retirement.rs"]
mod retirement;
#[path = "coordinator_retry.rs"]
mod retry;
#[path = "coordinator_slot.rs"]
mod slot;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Serving,
    Draining,
    Failed,
}

/// One process incarnation. Membership or authority invariant loss revokes it;
/// an unavailable renewal cancels only its still-live captured Agent authority,
/// and an unavailable member heartbeat skips one tick while membership holds.
pub struct OwnershipCoordinator {
    pub instance_id: String,
    pub store: Arc<dyn OwnershipStore>,
    policy: MaintenancePolicy,
    capacity: usize,
    cleanup_slots: tokio::sync::Semaphore,
    owned: std::sync::Mutex<BTreeMap<AgentId, slot::Slot>>,
    #[cfg(feature = "qualification-hooks")]
    qualification_renewal: std::sync::OnceLock<Arc<dyn crate::qualification::RenewalHook>>,
    acquisition: Mutex<()>,
    cancelled_cursor: AtomicUsize,
    member_deadline: std::sync::Mutex<Instant>,
    skipped_heartbeats: AtomicU32,
    draining: AtomicBool,
    lifecycle: watch::Sender<Lifecycle>,
    retry: Mutex<super::heartbeat_retry::RetryState>,
    failed: watch::Sender<bool>,
    batch_maintenance: Option<Mutex<()>>,
    pub(super) cleanup_pacing: super::cleanup_pacing::CleanupPacing,
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

    pub async fn register_with_cleanup_pacing(
        store: Arc<dyn OwnershipStore>,
        member: Member,
        enabled: bool,
    ) -> Result<Arc<Self>, OwnershipError> {
        Self::register_with_policy(store, member, enabled, MaintenancePolicy::Legacy).await
    }

    pub async fn register_with_policy(
        store: Arc<dyn OwnershipStore>,
        member: Member,
        cleanup_pacing: bool,
        policy: MaintenancePolicy,
    ) -> Result<Arc<Self>, OwnershipError> {
        let coordinator = Self::prepare_with_policy(store, member, cleanup_pacing, policy);
        coordinator.activate().await?;
        Ok(coordinator)
    }

    pub fn contains_failures(&self) -> bool {
        self.policy == MaintenancePolicy::ContainFailures
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

    pub async fn maintenance_stopped(&self) {
        let _ = self
            .lifecycle
            .subscribe()
            .wait_for(|state| *state != Lifecycle::Serving)
            .await;
    }

    /// Returns existing current authority. A new acquisition is declined with
    /// None, without a store call, while the latest heartbeat tick was skipped.
    pub async fn acquire(&self, id: &AgentId) -> Result<Option<Arc<Authority>>, OwnershipError> {
        if self.contains_failures() {
            self.acquire_contained(id).await
        } else {
            self.acquire_legacy(id).await
        }
    }

    pub async fn owned(&self) -> Vec<Arc<Authority>> {
        self.owned
            .lock()
            .unwrap()
            .values()
            .filter_map(|slot| slot.authority.clone())
            .collect()
    }

    pub async fn local(&self, id: &AgentId) -> Option<Arc<Authority>> {
        if !self.is_ready() {
            return None;
        }
        self.owned
            .lock()
            .unwrap()
            .get(id)
            .and_then(|slot| slot.authority.as_ref())
            .filter(|authority| authority.is_current())
            .cloned()
    }

    /// A successful renewal never revives an authority already cancelled locally.
    /// Returns cooperative handoff candidates; callers may defer busy Agents.
    /// A skipped heartbeat tick renews nothing and returns no candidates.
    pub async fn maintain(&self) -> Result<Vec<Arc<Authority>>, OwnershipError> {
        if self.contains_failures() {
            return self.maintain_step().await.map(|step| match step {
                super::MaintenanceStep::Confirmed { hints, .. } => hints,
                super::MaintenanceStep::RetryAt(_) | super::MaintenanceStep::Draining => Vec::new(),
            });
        }
        self.maintain_legacy().await
    }

    async fn maintain_legacy(&self) -> Result<Vec<Arc<Authority>>, OwnershipError> {
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
            return Ok(Vec::new());
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
        let owned: Vec<_> = self
            .owned
            .lock()
            .unwrap()
            .values()
            .filter_map(|slot| slot.authority.clone())
            .collect();
        if self.batch_maintenance.is_some() {
            let mut heartbeat = heartbeat::RenewalHeartbeat {
                started,
                membership: membership.clone(),
                member_deadline,
            };
            if !self.renew_in_batches(&owned, &mut heartbeat).await? {
                return Ok(Vec::new());
            }
        } else {
            let renewals = stream::iter(owned.clone())
                .map(|authority| async move {
                    self.renew_owned(&authority, started, membership.now_ms)
                        .await
                })
                .buffer_unordered(4)
                .try_collect::<Vec<_>>();
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(member_deadline) => {
                    return Err(self.maintenance_error("member_batch_deadline", None, OwnershipError::Stale));
                }
                result = renewals => { result?; }
            }
        }
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
        let owned = self.owned.lock().unwrap();
        let cancelled: Vec<_> = owned
            .values()
            .filter_map(|slot| slot.authority.as_ref())
            .filter(|a| {
                if self.contains_failures() {
                    a.retirement_pending()
                } else {
                    a.receiver_cancelled()
                }
            })
            .collect();
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
        {
            let _deadline = self.member_deadline.lock().unwrap();
            self.draining.store(true, Ordering::Release);
            self.lifecycle.send_if_modified(|state| {
                if *state == Lifecycle::Serving {
                    *state = Lifecycle::Draining;
                    true
                } else {
                    false
                }
            });
        }
        if self.contains_failures() {
            for authority in self
                .owned
                .lock()
                .unwrap()
                .values()
                .filter_map(|slot| slot.authority.as_ref())
            {
                authority.revoke();
            }
        }
    }

    pub async fn begin_drain(&self) -> Vec<Arc<Authority>> {
        if self
            .prepare_drain(Instant::now() + Duration::from_secs(23))
            .await
            .is_err()
        {
            self.fail().await;
        }
        let owned = self.owned().await;
        for authority in &owned {
            authority.revoke();
        }
        owned
    }

    pub async fn fail(&self) {
        {
            let _deadline = self.member_deadline.lock().unwrap();
            self.failed.send_replace(true);
            self.lifecycle.send_replace(Lifecycle::Failed);
        }
        for authority in self
            .owned
            .lock()
            .unwrap()
            .values()
            .filter_map(|slot| slot.authority.as_ref())
        {
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
    let until = started
        .checked_add(Duration::from_millis(remaining as u64))
        .ok_or(OwnershipError::Stale)?;
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
