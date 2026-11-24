//! Membership stays dormant until the process can serve the work it would own.
//!
//! A prepared coordinator holds its member record without registering it. No
//! peer can place an Agent on it, invite it to claim one or observe it in the
//! membership while startup is still proving it can serve. Activation registers
//! the record once; termination before activation leaves nothing registered.
use super::*;

impl OwnershipCoordinator {
    /// Prepare membership without store IO. Until `activate` succeeds the
    /// coordinator is not ready and acquires nothing.
    pub fn prepare(store: Arc<dyn OwnershipStore>, member: Member) -> Arc<Self> {
        Self::prepare_with_cleanup_pacing(store, member, false)
    }

    pub fn prepare_with_cleanup_pacing(
        store: Arc<dyn OwnershipStore>,
        member: Member,
        enabled: bool,
    ) -> Arc<Self> {
        Self::prepare_with_policy(store, member, enabled, MaintenancePolicy::Legacy)
    }

    pub fn prepare_with_policy(
        store: Arc<dyn OwnershipStore>,
        member: Member,
        cleanup_pacing: bool,
        policy: MaintenancePolicy,
    ) -> Arc<Self> {
        Self::prepare_with_policy_options(store, member, cleanup_pacing, policy, false)
    }

    /// Prepare membership with both the failure policy and optional batch renewal.
    pub fn prepare_with_policy_options(
        store: Arc<dyn OwnershipStore>,
        member: Member,
        cleanup_pacing: bool,
        policy: MaintenancePolicy,
        batch_renewal: bool,
    ) -> Arc<Self> {
        let capacity = member.max_owned_agents as usize;
        Arc::new(Self {
            instance_id: member.instance_id.clone(),
            batch_maintenance: (batch_renewal && store.supports_batch_renewal())
                .then(Mutex::default),
            store,
            policy,
            capacity,
            cleanup_slots: tokio::sync::Semaphore::new(4),
            owned: std::sync::Mutex::new(BTreeMap::new()),
            #[cfg(feature = "qualification-hooks")]
            qualification_renewal: std::sync::OnceLock::new(),
            acquisition: Mutex::new(()),
            cancelled_cursor: AtomicUsize::new(0),
            // Already passed: nothing is ready until registration sets it.
            member_deadline: std::sync::Mutex::new(Instant::now()),
            skipped_heartbeats: AtomicU32::new(0),
            draining: AtomicBool::new(false),
            lifecycle: watch::channel(Lifecycle::Serving).0,
            retry: Mutex::new(super::super::heartbeat_retry::RetryState::new()),
            failed: watch::channel(false).0,
            cleanup_pacing: super::super::cleanup_pacing::CleanupPacing::new(
                cleanup_pacing,
                capacity,
            ),
            registration: std::sync::Mutex::new(Some(member)),
            registered: AtomicBool::new(false),
        })
    }

    /// Develop's batch-renewal entry point retains the legacy failure policy.
    pub fn prepare_with_options(
        store: Arc<dyn OwnershipStore>,
        member: Member,
        cleanup_pacing: bool,
        batch_renewal: bool,
    ) -> Arc<Self> {
        Self::prepare_with_policy_options(
            store,
            member,
            cleanup_pacing,
            MaintenancePolicy::Legacy,
            batch_renewal,
        )
    }

    pub fn is_registered(&self) -> bool {
        self.registered.load(Ordering::Acquire)
    }

    /// Register the prepared member. Serialized with acquisition and drain, so
    /// admission closed before registration registers nothing. A registration
    /// whose outcome is unknown fails the process; any committed row expires.
    pub async fn activate(&self) -> Result<(), OwnershipError> {
        let _acquisition = self.acquisition.lock().await;
        if self.is_draining() || *self.failed.borrow() {
            return Err(OwnershipError::Stale);
        }
        if self.is_registered() {
            return self.is_ready().then_some(()).ok_or(OwnershipError::Stale);
        }
        let member = self
            .registration
            .lock()
            .unwrap()
            .take()
            .ok_or(OwnershipError::Stale)?;
        let started = Instant::now();
        let registered = async {
            let membership = bounded(self.store.register(member)).await?;
            let member = membership
                .members
                .iter()
                .find(|member| member.instance_id == self.instance_id)
                .ok_or(OwnershipError::Stale)?;
            deadline(started, membership.now_ms, member.expires_at_ms)
        }
        .await;
        match registered {
            Ok(deadline) => {
                *self.member_deadline.lock().unwrap() = deadline;
                self.registered.store(true, Ordering::Release);
                Ok(())
            }
            Err(error) => {
                self.failed.send_replace(true);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
#[path = "coordinator_activation_tests.rs"]
mod tests;
