//! Dormant membership until the composition proves it can serve.
use super::*;

struct ActivationAttempt<'a> {
    failed: &'a watch::Sender<bool>,
    complete: bool,
}

impl Drop for ActivationAttempt<'_> {
    fn drop(&mut self) {
        if !self.complete {
            // Dropping registration cannot prove that the store did not commit.
            self.failed.send_replace(true);
        }
    }
}

impl OwnershipCoordinator {
    /// Register immediately when a caller has already proved serving readiness.
    /// Application composition uses `prepare` and activates after startup.
    pub async fn register(
        store: Arc<dyn OwnershipStore>,
        member: Member,
    ) -> Result<Arc<Self>, OwnershipError> {
        let coordinator = Self::prepare(store, member);
        coordinator.activate().await?;
        Ok(coordinator)
    }

    pub fn prepare(store: Arc<dyn OwnershipStore>, member: Member) -> Arc<Self> {
        let max_pending_acquisitions = member.max_owned_agents as usize;
        Arc::new(Self {
            instance_id: member.instance_id.clone(),
            store,
            owned: Mutex::new(BTreeMap::new()),
            acquisition: Mutex::new(()),
            maintenance: Mutex::new(()),
            registration: std::sync::Mutex::new(Some(member)),
            registered: AtomicBool::new(false),
            registration_pending: AtomicBool::new(false),
            admission_readiness: OnceLock::new(),
            pending_acquisitions: std::sync::Mutex::default(),
            max_pending_acquisitions,
            cancelled_cursor: AtomicUsize::new(0),
            member_deadline: std::sync::Mutex::new(Instant::now()),
            draining: AtomicBool::new(false),
            failed: watch::channel(false).0,
        })
    }

    pub fn is_starting(&self) -> bool {
        !self.registered.load(Ordering::Acquire) && !self.is_draining() && !*self.failed.borrow()
    }

    /// Independent application prerequisites, shared with public readiness.
    /// Membership must not be part of this predicate: that creates a startup cycle.
    pub fn set_admission_readiness(
        &self,
        ready: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<(), OwnershipError> {
        if self.registered.load(Ordering::Acquire)
            || self.registration_pending.load(Ordering::Acquire)
        {
            return Err(OwnershipError::Stale);
        }
        self.admission_readiness
            .set(ready)
            .map_err(|_| OwnershipError::Stale)
    }

    fn admission_ready(&self) -> bool {
        self.admission_readiness.get().is_none_or(|ready| ready())
    }

    pub fn can_acquire(&self) -> bool {
        self.is_ready() && self.admission_ready()
    }

    pub fn uncertain_acquisitions(&self) -> usize {
        self.pending_acquisitions.lock().unwrap().len()
    }

    pub fn uncertain_registration(&self) -> bool {
        self.registration_pending.load(Ordering::Acquire)
    }

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
        self.registration_pending.store(true, Ordering::Release);
        let mut attempt = ActivationAttempt {
            failed: &self.failed,
            complete: false,
        };
        let started = Instant::now();
        let membership = bounded(self.store.register(member)).await?;
        membership.require_compatible()?;
        let member = membership
            .members
            .iter()
            .find(|member| member.instance_id == self.instance_id)
            .ok_or(OwnershipError::Stale)?;
        *self.member_deadline.lock().unwrap() =
            deadline(started, membership.now_ms, member.expires_at_ms)?;
        self.registered.store(true, Ordering::Release);
        self.registration_pending.store(false, Ordering::Release);
        if self.is_draining() {
            let drained = bounded(self.store.heartbeat(&self.instance_id, true)).await?;
            drained.require_compatible()?;
            if !self.member_is_live()
                || !drained.members.iter().any(|member| {
                    member.instance_id == self.instance_id
                        && member.draining
                        && member.is_live(drained.now_ms)
                })
            {
                return Err(OwnershipError::Stale);
            }
        }
        attempt.complete = true;
        // Drain joins this lock and advertises the terminal state. A late
        // registration reply grants no execution when the serving proof is lost.
        self.can_acquire()
            .then_some(())
            .ok_or(OwnershipError::Stale)
    }
}

#[cfg(test)]
mod tests;
