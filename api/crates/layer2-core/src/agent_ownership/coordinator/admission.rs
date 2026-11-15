//! Dormant construction, readiness admission and activation notification.
use super::*;

impl OwnershipCoordinator {
    pub async fn register(
        store: Arc<dyn OwnershipStore>,
        member: Member,
    ) -> Result<Arc<Self>, OwnershipError> {
        let coordinator = Self::prepare(store, member);
        coordinator.activate().await?;
        Ok(coordinator)
    }

    /// Construct an observer without advertising placement or admitting work.
    /// Composition activates only after its serving prerequisites are proved.
    pub fn prepare(store: Arc<dyn OwnershipStore>, member: Member) -> Arc<Self> {
        Arc::new(Self {
            instance_id: member.instance_id.clone(),
            store,
            owned: Mutex::new(BTreeMap::new()),
            acquisition: Mutex::new(()),
            registration: std::sync::Mutex::new(Some(member)),
            registered: AtomicBool::new(false),
            admission_readiness: OnceLock::new(),
            uncertain_acquisitions: AtomicUsize::new(0),
            cancelled_cursor: AtomicUsize::new(0),
            member_deadline: std::sync::Mutex::new(Instant::now()),
            draining: AtomicBool::new(false),
            failed: watch::channel(false).0,
        })
    }

    pub fn is_starting(&self) -> bool {
        !self.registered.load(Ordering::Acquire) && !self.is_draining() && !*self.failed.borrow()
    }

    /// Composition supplies its live serving proof before activation. This
    /// gates acquisition without interrupting renewal of retained authorities.
    pub fn set_admission_readiness(
        &self,
        ready: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<(), OwnershipError> {
        if self.registered.load(Ordering::Acquire) {
            return Err(OwnershipError::Stale);
        }
        self.admission_readiness
            .set(ready)
            .map_err(|_| OwnershipError::Stale)
    }

    pub(super) fn admission_ready(&self) -> bool {
        self.admission_readiness.get().is_none_or(|ready| ready())
    }

    /// The live readiness used for admitting ownership. Public serving health
    /// must use this same predicate, not membership liveness alone.
    pub fn can_acquire(&self) -> bool {
        self.is_ready() && self.admission_ready()
    }

    /// Registration is single-use, including uncertain store responses.
    pub async fn activate(&self) -> Result<(), OwnershipError> {
        let _acquisition = self.acquisition.lock().await;
        if self.is_draining() || *self.failed.borrow() || !self.admission_ready() {
            return Err(OwnershipError::Stale);
        }
        if self.registered.load(Ordering::Acquire) {
            return self.is_ready().then_some(()).ok_or(OwnershipError::Stale);
        }
        let member = self
            .registration
            .lock()
            .unwrap()
            .take()
            .ok_or(OwnershipError::Stale)?;
        let started = Instant::now();
        let result = async {
            let membership = bounded(self.store.register(member)).await?;
            let member = membership
                .members
                .iter()
                .find(|member| member.instance_id == self.instance_id)
                .ok_or(OwnershipError::Stale)?;
            let deadline = deadline(started, membership.now_ms, member.expires_at_ms)?;
            *self.member_deadline.lock().unwrap() = deadline;
            self.registered.store(true, Ordering::Release);
            if self.is_draining() || *self.failed.borrow() {
                let _ = bounded(self.store.heartbeat(&self.instance_id, true)).await;
                return Err(OwnershipError::Stale);
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            self.failed.send_replace(true);
        }
        result
    }
}

#[cfg(test)]
mod tests;
