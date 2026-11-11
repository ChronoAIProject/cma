use super::*;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::TypedFailure;
use layer3_adapter::agent_sandbox::NativePoolControl;

struct FailingStore {
    inner: Arc<InMemorySandboxStore>,
    held: SandboxId,
    calls: AtomicUsize,
    fail_pause: AtomicBool,
}
#[async_trait::async_trait]
impl SandboxStore for FailingStore {
    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        self.inner.create_sandbox(sandbox).await
    }
    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        if id == &self.held {
            self.calls.fetch_add(1, Ordering::SeqCst);
            return Err(SandboxStoreError::Unavailable(
                "fixture binding cannot be read".into(),
            ));
        }
        self.inner.get_sandbox(id).await
    }
    async fn save_sandbox(&self, revision: u64, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
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
impl SandboxReconciliationStore for FailingStore {
    async fn sandbox_runtime_authority(
        &self,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        self.inner.sandbox_runtime_authority(page).await
    }
    async fn recover_runtime_loss(
        &self,
        request: SandboxRuntimeLossRequest,
    ) -> Result<SandboxRuntimeLossOutcome, SandboxStoreError> {
        self.inner.recover_runtime_loss(request).await
    }
}
#[async_trait::async_trait]
impl SandboxMaintenanceStore for FailingStore {
    async fn sandbox_maintenance_candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<SandboxId>, SandboxStoreError> {
        self.inner.sandbox_maintenance_candidates(page).await
    }
    async fn schedule_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        if self.fail_pause.load(Ordering::SeqCst) {
            return Err(SandboxStoreError::Unavailable(
                "fixture pause is unavailable".into(),
            ));
        }
        self.inner.schedule_inactivity_pause(request).await
    }
    async fn preview_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        self.inner.preview_inactivity_pause(request).await
    }
    async fn schedule_image_rotation(
        &self,
        request: SandboxImageRotationRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        self.inner.schedule_image_rotation(request).await
    }
    async fn set_sandbox_always_on(
        &self,
        id: SandboxId,
        revision: u64,
        value: bool,
    ) -> Result<Sandbox, SandboxStoreError> {
        self.inner.set_sandbox_always_on(id, revision, value).await
    }
}

#[tokio::test]
async fn permanently_failed_early_binding_does_not_starve_a_real_native_boot_replacement() {
    let mut fixture = Fixture::native().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    fixture.harness.adapter = fixture.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    let agent = lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 1).await;
    now = super::after_durable_boundary(&active).await;
    let mut early = created.clone();
    early.id = SandboxId("sbx_000_held".into());
    assert!(early.id < active.id);
    early.name = "held".into();
    early.status = SandboxStatus::Failed;
    early.failure = Some(TypedFailure {
        class: "fixture".into(),
        detail: "fixture".into(),
    });
    early
        .work
        .schedule(SandboxWorkStep::None, early.updated_at.clone());
    early.validate().unwrap();
    fixture
        .harness
        .store
        .create_sandbox(early.clone())
        .await
        .unwrap();
    let store = Arc::new(FailingStore {
        inner: fixture.harness.store.clone(),
        held: early.id.clone(),
        calls: AtomicUsize::new(0),
        fail_pause: AtomicBool::new(false),
    });
    let reconciler = SandboxReconciler::new(
        store.clone(),
        fixture.adapter(&runtime_image()),
        Arc::new(fixture.harness.policy.clone()),
        "test".into(),
    );
    // An ordinary fresh API observer adopts without changing authority.
    assert!(reconciler.run_due(now).await.unwrap());
    assert_eq!(fixture.harness.stored(&active.id).await, active);
    fixture.backend.restart_process().await;
    assert!(reconciler.run_due(now).await.unwrap());
    let recovered = fixture.harness.stored(&active.id).await;
    assert_eq!(recovered.status, SandboxStatus::Reviving);
    assert_eq!(recovered.lifecycle.home, active.lifecycle.home);
    assert_eq!(
        recovered.lifecycle.suspended_runtime.as_deref(),
        active.runtime.as_ref()
    );
    let recovered_agent = fixture
        .harness
        .store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered_agent.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(recovered_agent.status, AgentStatus::Paused);
    assert_eq!(fixture.harness.stored(&early.id).await, early);
    assert_eq!(
        store.calls.load(Ordering::SeqCst),
        1,
        "subsequent sweeps respect hold backoff"
    );
    assert!(!fixture
        .backend
        .calls
        .snapshot()
        .contains(&"delete-retained".into()));
    // Initialization reads the complete authority snapshot directly. It can
    // adopt the remaining retained authority without changing recovery state.
    SandboxReconciler::new(
        store.clone(),
        fixture.adapter(&runtime_image()),
        Arc::new(fixture.harness.policy.clone()),
        "test".into(),
    )
    .initialize(now)
    .await
    .unwrap();
    assert_eq!(fixture.harness.stored(&active.id).await, recovered);
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn held_native_binding_never_reaches_claim_home_or_runtime_reclamation() {
    let fixture = Fixture::native().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    let before = fixture.backend.calls.snapshot();
    let store = Arc::new(FailingStore {
        inner: fixture.harness.store.clone(),
        held: active.id.clone(),
        calls: AtomicUsize::new(0),
        fail_pause: AtomicBool::new(false),
    });
    let reconciler = SandboxReconciler::new(
        store.clone(),
        fixture.adapter(&runtime_image()),
        Arc::new(fixture.harness.policy.clone()),
        "test".into(),
    );
    assert!(reconciler.run_due(now).await.unwrap());
    assert_eq!(
        store.calls.load(Ordering::SeqCst),
        1,
        "claim/home/runtime cleanup must not reread and then act on a held row"
    );
    assert_eq!(fixture.harness.stored(&active.id).await, active);
    assert_eq!(fixture.backend.calls.snapshot(), before);
    assert!(
        fixture
            .backend
            .pools
            .as_ref()
            .unwrap()
            .inventory_claims()
            .await
            .unwrap()
            .len()
            == 1
    );
}

#[tokio::test]
async fn an_ordinary_binding_detects_later_boot_loss_behind_a_persistent_pause_fault() {
    let mut fixture = Fixture::native().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    fixture.harness.adapter = fixture.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    let agent = lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 1).await;
    now = super::after_durable_boundary(&active).await;
    let store = Arc::new(FailingStore {
        inner: fixture.harness.store.clone(),
        held: SandboxId("sbx_not_present".into()),
        calls: AtomicUsize::new(0),
        fail_pause: AtomicBool::new(true),
    });
    let reconciler = SandboxReconciler::new(
        store,
        fixture.adapter(&runtime_image()),
        Arc::new(fixture.harness.policy.clone()),
        "test".into(),
    );
    assert!(reconciler.run_due(now).await.is_err());
    assert_eq!(fixture.harness.stored(&active.id).await, active);
    fixture.backend.restart_process().await;
    assert!(reconciler.run_due(now).await.is_err());
    let recovered = fixture.harness.stored(&active.id).await;
    assert_eq!(recovered.status, SandboxStatus::Reviving);
    assert_eq!(recovered.lifecycle.home, active.lifecycle.home);
    assert_eq!(
        recovered.lifecycle.suspended_runtime.as_deref(),
        active.runtime.as_ref()
    );
    let paused = fixture
        .harness
        .store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(paused.status, AgentStatus::Paused);
    assert_eq!(
        paused.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert!(reconciler.run_due(now).await.is_err());
    assert_eq!(fixture.harness.stored(&active.id).await, recovered);
    assert!(!fixture
        .backend
        .calls
        .snapshot()
        .contains(&"delete-retained".into()));
}
