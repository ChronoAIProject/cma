use super::*;
use layer2_core::sandbox::store::*;

pub(super) struct Store {
    pub inner: InMemorySandboxStore,
    pub control: Arc<Control>,
    pub native: SandboxId,
}

#[async_trait::async_trait]
impl SandboxStore for Store {
    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        self.inner.create_sandbox(sandbox).await
    }
    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        if *id == self.native && self.control.cleanup.load(Ordering::SeqCst) == 0 {
            self.control.record("DURABLE_READ", id.as_str());
            self.control.fenced.notify_one();
        }
        self.inner.get_sandbox(id).await
    }
    async fn save_sandbox(&self, revision: u64, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        if sandbox.id == self.native {
            self.control.record("DURABLE_CAS", sandbox.id.as_str());
            self.control.fenced.notify_one();
        }
        self.inner.save_sandbox(revision, sandbox).await
    }
    async fn list_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        self.inner.list_sandboxes(owner, page).await
    }
}

#[async_trait::async_trait]
impl SandboxReconciliationStore for Store {
    async fn sandbox_runtime_authority(
        &self,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        self.inner.sandbox_runtime_authority(page).await
    }
    async fn recover_runtime_loss(
        &self,
        _: SandboxRuntimeLossRequest,
    ) -> Result<SandboxRuntimeLossOutcome, SandboxStoreError> {
        panic!("startup fixture must retain every bound runtime")
    }
    async fn unowned_runtime_authority(
        &self,
        id: &SandboxId,
        _: u64,
        _: &str,
    ) -> Result<bool, SandboxStoreError> {
        self.control.assert_siblings_settled();
        self.control.cleanup.fetch_add(1, Ordering::SeqCst);
        self.control.record("ORPHAN_PHASE", id.as_str());
        Ok(false)
    }
}

#[async_trait::async_trait]
impl SandboxMaintenanceStore for Store {
    async fn sandbox_maintenance_candidates(
        &self,
        _: &PageRequest,
    ) -> Result<Page<SandboxId>, SandboxStoreError> {
        assert!(self.control.background_sweep.load(Ordering::SeqCst));
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }
    async fn schedule_inactivity_pause(
        &self,
        _: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        panic!("startup must not pause a runtime")
    }
    async fn preview_inactivity_pause(
        &self,
        _: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        panic!("startup must not preview maintenance")
    }
    async fn set_sandbox_always_on(
        &self,
        _: SandboxId,
        _: u64,
        _: bool,
    ) -> Result<Sandbox, SandboxStoreError> {
        panic!("startup must not change owner policy")
    }
    async fn schedule_image_rotation(
        &self,
        _: SandboxImageRotationRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        panic!("startup must not rotate a runtime")
    }
}
