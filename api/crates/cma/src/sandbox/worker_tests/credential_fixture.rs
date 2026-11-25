use super::*;

pub(crate) struct MemoryCredentials {
    pub(super) live_keys: Mutex<std::collections::BTreeSet<String>>,
    pub(super) revoke_failure_ids: Mutex<std::collections::BTreeSet<String>>,
    pub(super) census_failure: Mutex<Option<CredentialError>>,
    pub(super) settlement_failure: Mutex<Option<CredentialError>>,
    pub(super) materials: Mutex<BTreeMap<(SandboxId, u64), CredentialMaterial>>,
    pub(super) issue_failures: Mutex<VecDeque<CredentialError>>,
    pub(super) revoke_failures: AtomicUsize,
    pub(super) forget_failures: AtomicUsize,
    pub(super) expiry_seconds: AtomicU64,
    pub(super) actions: ActionLog,
}

impl MemoryCredentials {
    pub(crate) fn new(actions: ActionLog) -> Arc<Self> {
        Arc::new(Self {
            live_keys: Default::default(),
            revoke_failure_ids: Default::default(),
            census_failure: Default::default(),
            settlement_failure: Default::default(),
            materials: Mutex::new(BTreeMap::new()),
            issue_failures: Mutex::new(VecDeque::new()),
            revoke_failures: AtomicUsize::new(0),
            forget_failures: AtomicUsize::new(0),
            expiry_seconds: AtomicU64::new(30 * 24 * 60 * 60),
            actions,
        })
    }

    pub(super) fn fail_issue(&self, failure: CredentialError) {
        self.issue_failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(failure);
    }

    pub(super) fn fail_revokes(&self, count: usize) {
        self.revoke_failures.store(count, Ordering::SeqCst);
    }

    pub(super) fn fail_forgets(&self, count: usize) {
        self.forget_failures.store(count, Ordering::SeqCst);
    }

    pub(super) fn restore(&self, material: CredentialMaterial) {
        self.live_keys
            .lock()
            .unwrap()
            .insert(material.authority.provider_key_id.clone());
        self.materials
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                (material.sandbox_id.clone(), material.authority.generation),
                material,
            );
    }

    pub(super) fn expire_after(&self, duration: Duration) {
        self.expiry_seconds
            .store(duration.as_secs(), Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl SandboxCredentialController for MemoryCredentials {
    async fn issue(
        &self,
        sandbox: &Sandbox,
        generation: u64,
        now: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        if let Some(failure) = self
            .issue_failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
        {
            return Err(failure);
        }
        let key = (sandbox.id.clone(), generation);
        let mut materials = self
            .materials
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(material) = materials.get(&key) {
            return Ok(material.clone());
        }
        self.actions.push(format!("mint:{generation}"));
        let material = CredentialMaterial {
            source: Default::default(),
            github_route: None,
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            authority: layer2_core::sandbox::SandboxCredentialAuthority {
                generation,
                provider_key_id: format!("provider-key-{generation}"),
                provider_state_version: i64::try_from(generation).unwrap(),
                issued_at: rfc3339_at(now),
                expires_at: rfc3339_at(
                    now + Duration::from_secs(self.expiry_seconds.load(Ordering::SeqCst)),
                ),
                grant_fingerprint: format!("{generation:064x}"),
            },
            value: SecretValue::new(format!("one-time-secret-{generation}")),
        };
        self.live_keys
            .lock()
            .unwrap()
            .insert(material.authority.provider_key_id.clone());
        materials.insert(key, material.clone());
        Ok(material)
    }

    async fn material(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        Ok(self
            .materials
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&(sandbox_id.clone(), generation))
            .cloned())
    }

    async fn custodied(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialError> {
        if let Some(error) = self.census_failure.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(self
            .materials
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|((id, _), _)| id == sandbox_id)
            .map(|(_, material)| material.clone())
            .collect())
    }

    async fn forget(&self, sandbox_id: &SandboxId, generation: u64) -> Result<(), CredentialError> {
        if self
            .forget_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(CredentialError::Unavailable(
                "simulated custody delete outage".into(),
            ));
        }
        self.materials
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&(sandbox_id.clone(), generation));
        Ok(())
    }

    async fn revoke(
        &self,
        _sandbox: &Sandbox,
        provider_key_id: &str,
    ) -> Result<(), CredentialError> {
        self.actions.push(format!("revoke:{provider_key_id}"));
        if self
            .revoke_failure_ids
            .lock()
            .unwrap()
            .contains(provider_key_id)
        {
            return Err(CredentialError::Unavailable(
                "simulated key revocation outage".into(),
            ));
        }
        if self
            .revoke_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            Err(CredentialError::Unavailable(
                "simulated revoke outage".into(),
            ))
        } else {
            self.live_keys.lock().unwrap().remove(provider_key_id);
            Ok(())
        }
    }
    async fn settle_revocations(&self, _: &Sandbox) -> Result<(), CredentialError> {
        match self.settlement_failure.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

pub(super) struct BlockingCredentials {
    pub(super) inner: Arc<MemoryCredentials>,
    pub(super) block_first: AtomicBool,
    pub(super) entered: Notify,
    pub(super) release: Notify,
}

impl BlockingCredentials {
    pub(super) fn new(inner: Arc<MemoryCredentials>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            block_first: AtomicBool::new(true),
            entered: Notify::new(),
            release: Notify::new(),
        })
    }
}

#[async_trait::async_trait]
impl SandboxCredentialController for BlockingCredentials {
    async fn issue(
        &self,
        sandbox: &Sandbox,
        generation: u64,
        now: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        if self.block_first.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.issue(sandbox, generation, now).await
    }

    async fn material(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        self.inner.material(sandbox_id, generation).await
    }

    async fn custodied(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialError> {
        self.inner.custodied(sandbox_id).await
    }

    async fn forget(&self, sandbox_id: &SandboxId, generation: u64) -> Result<(), CredentialError> {
        self.inner.forget(sandbox_id, generation).await
    }

    async fn revoke(
        &self,
        sandbox: &Sandbox,
        provider_key_id: &str,
    ) -> Result<(), CredentialError> {
        self.inner.revoke(sandbox, provider_key_id).await
    }
}
