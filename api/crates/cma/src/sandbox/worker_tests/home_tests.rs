use super::*;
use layer3_adapter::{
    AdapterError, HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest,
    HomeVolumeStore,
};

pub(super) fn home_policy() -> HomeVolumePolicy {
    HomeVolumePolicy {
        storage_class: "storage".into(),
        capacity_gib: 64,
        template_name: "seed".into(),
        template_generation: "seed-generation".into(),
    }
}

pub(super) struct Homes {
    authority: Arc<InMemorySandboxStore>,
    backend: InMemoryHostedBackend,
    scope: layer3_adapter::HomeVolumeScope,
    disks: Mutex<BTreeMap<String, HomeVolumeAttachment>>,
    prepare_calls: AtomicUsize,
    lose_response: AtomicBool,
    block_prepare: AtomicBool,
    entered: Notify,
    resume: Notify,
}

impl Homes {
    pub(super) fn new(harness: &Harness) -> Arc<Self> {
        Self::for_cluster(harness, "local")
    }
    fn for_cluster(harness: &Harness, cluster: &str) -> Arc<Self> {
        Arc::new(Self {
            scope: layer3_adapter::HomeVolumeScope::from_authority(
                "kubernetes-pvc-v1",
                &[cluster, "runtime", "deployment"],
            )
            .unwrap(),
            authority: harness.store.clone(),
            backend: harness.backend.clone(),
            disks: Mutex::new(BTreeMap::new()),
            prepare_calls: AtomicUsize::new(0),
            lose_response: AtomicBool::new(false),
            block_prepare: AtomicBool::new(false),
            entered: Notify::new(),
            resume: Notify::new(),
        })
    }
}

#[async_trait::async_trait]
impl HomeVolumeStore for Homes {
    fn allocation_scope(&self) -> Option<layer3_adapter::HomeVolumeScope> {
        Some(self.scope.clone())
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(self.disks.lock().unwrap().values().cloned().collect())
    }
    async fn prepare(
        &self,
        request: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<HomeVolumeReceipt> {
        let stored = self
            .authority
            .get_sandbox(&SandboxId(request.sandbox_id.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.lifecycle.home.as_ref().unwrap().request,
            *request,
            "home intent must be durable before allocation"
        );
        assert!(stored.lifecycle.home.as_ref().unwrap().receipt.is_none());
        self.prepare_calls.fetch_add(1, Ordering::SeqCst);
        if self.block_prepare.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        let attachment = HomeVolumeAttachment {
            request: request.clone(),
            receipt: HomeVolumeReceipt {
                volume_id: request.volume_id.clone(),
                request_sha256: request.computed_sha256(),
                storage_reference: "fixture-home".into(),
                storage_generation: "original-disk".into(),
            },
        };
        let receipt = self
            .disks
            .lock()
            .unwrap()
            .entry(request.volume_id.clone())
            .or_insert(attachment)
            .receipt
            .clone();
        if self.lose_response.swap(false, Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "allocated but response was lost".into(),
            ));
        }
        Ok(receipt)
    }
    async fn verify(&self, attachment: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        let stored = self
            .authority
            .get_sandbox(&SandboxId(attachment.request.sandbox_id.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored
                .lifecycle
                .home
                .as_ref()
                .unwrap()
                .attachment()
                .as_ref(),
            Some(attachment),
            "receipt must be durable before mounting or credentials"
        );
        if self
            .disks
            .lock()
            .unwrap()
            .get(&attachment.request.volume_id)
            == Some(attachment)
        {
            Ok(())
        } else {
            Err(AdapterError::Rejected("original disk is missing".into()))
        }
    }
    async fn remove(
        &self,
        request: &HomeVolumeRequest,
        receipt: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        use layer3_adapter::hosted::HostedBackend;
        assert!(
            self.backend
                .inventory()
                .await
                .unwrap()
                .iter()
                .all(|row| row.sandbox_id != request.sandbox_id),
            "disk must remain until its runtime is gone"
        );
        let mut disks = self.disks.lock().unwrap();
        if let Some(disk) = disks.get(&request.volume_id) {
            assert_eq!(disk.request, *request);
            if let Some(receipt) = receipt {
                assert_eq!(disk.receipt, *receipt);
            }
        }
        disks.remove(&request.volume_id);
        Ok(())
    }
}

pub(super) fn adapter(harness: &Harness, homes: Arc<Homes>) -> Arc<dyn SandboxAdapter> {
    Arc::new(
        CodexAdapter::with_backend(harness.backend.clone(), CodexSandboxSpec::default())
            .with_home_volumes(homes),
    )
}

pub(super) fn worker(harness: &Harness, homes: Arc<Homes>, holder: &str) -> Arc<SandboxWorker> {
    worker_with_adapter(harness, adapter(harness, homes), holder)
}

fn worker_with_adapter(
    harness: &Harness,
    adapter: Arc<dyn SandboxAdapter>,
    holder: &str,
) -> Arc<SandboxWorker> {
    SandboxWorker::new(
        SandboxWorkerDependencies {
            store: harness.store.clone(),
            adapter,
            credentials: harness.credentials.clone(),
            workspaces: Arc::new(WorkspaceRecorder {
                actions: harness.actions.clone(),
            }),
            agents: Arc::new(AgentStopRecorder {
                actions: harness.actions.clone(),
            }),
            registry: harness.registry.clone(),
            updates: DurableAgentUpdateRecorder::new(harness.store.clone()),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        },
        SandboxWorkerConfig::new(holder, "test", runtime_image(), harness.policy.clone())
            .with_home_policy(Some(home_policy())),
    )
}

#[tokio::test]
async fn lost_allocation_response_restarts_with_the_same_disk_before_install_or_credentials() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let homes = Homes::new(&harness);
    homes.lose_response.store(true, Ordering::SeqCst);
    worker(&harness, homes.clone(), "first")
        .run_one(&created.id, now)
        .await
        .unwrap();
    let interrupted = harness.stored(&created.id).await;
    assert_eq!(interrupted.status, SandboxStatus::Provisioning);
    assert!(interrupted
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .receipt
        .is_none());
    assert!(harness.actions.snapshot().is_empty());
    let disk = homes.inventory().await.unwrap().pop().unwrap();
    let restarted = worker(&harness, homes.clone(), "restarted");
    for _ in 0..5 {
        restarted
            .run_one(&created.id, now + Duration::from_secs(31))
            .await
            .unwrap();
    }
    let active = harness.stored(&created.id).await;
    assert_eq!(active.status, SandboxStatus::Active);
    assert_eq!(
        active.lifecycle.home.unwrap().attachment(),
        Some(disk.clone())
    );
    assert_eq!(homes.prepare_calls.load(Ordering::SeqCst), 2);
    assert_eq!(homes.inventory().await.unwrap(), [disk]);
    let snapshot = harness
        .backend
        .snapshot(&RuntimeHandle::new(&active.runtime.unwrap().runtime_id))
        .await
        .unwrap();
    assert_eq!(snapshot.request.volumes.len(), 3);
    assert!(snapshot.credential_delivered);
}

#[tokio::test]
async fn deletion_wins_a_delayed_allocation_and_reconciliation_releases_its_late_disk() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let homes = Homes::new(&harness);
    homes.block_prepare.store(true, Ordering::SeqCst);
    let slow = worker(&harness, homes.clone(), "slow");
    let id = created.id.clone();
    let job = tokio::spawn(async move { slow.run_one(&id, now).await });
    tokio::time::timeout(Duration::from_secs(2), homes.entered.notified())
        .await
        .unwrap();
    harness
        .service
        .delete(
            &caller("owner", [Capability::DeleteOwnSandbox]),
            &created.id,
        )
        .await
        .unwrap();
    let deleting = worker(&harness, homes.clone(), "deleting");
    for _ in 0..4 {
        deleting
            .run_one(&created.id, now + Duration::from_secs(1))
            .await
            .unwrap();
    }
    let retired = harness.stored(&created.id).await;
    assert_eq!(retired.status, SandboxStatus::Retired);
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    homes.resume.notify_one();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), job)
            .await
            .unwrap()
            .unwrap(),
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(homes.inventory().await.unwrap().len(), 1);
    crate::sandbox::reconciliation::SandboxReconciler::new(
        harness.store.clone(),
        adapter(&harness, homes.clone()),
        Arc::new(harness.policy.clone()),
        "test".into(),
    )
    .initialize(now + Duration::from_secs(2))
    .await
    .unwrap();
    assert!(homes.inventory().await.unwrap().is_empty());
    assert_eq!(harness.stored(&created.id).await, retired);
    assert!(harness
        .actions
        .snapshot()
        .iter()
        .all(|action| !action.starts_with("mint:")));
}

#[tokio::test]
async fn process_identity_is_recorded_before_install_and_changed_process_cannot_receive_credentials(
) {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let homes = Homes::new(&harness);
    let worker = worker(&harness, homes.clone(), "boot-worker");
    let mut stored = harness.stored(&created.id).await;
    for _ in 0..5 {
        if stored.runtime.is_some() {
            break;
        }
        worker.run_one(&created.id, now).await.unwrap();
        stored = harness.stored(&created.id).await;
    }
    let binding = stored.runtime.as_ref().unwrap();
    assert!(binding.boot_id.is_some());
    assert_eq!(stored.work.step, SandboxWorkStep::RunInstall);
    assert!(harness.actions.snapshot().is_empty());
    let handle = RuntimeHandle::new(&binding.runtime_id);
    let adopter = adapter(&harness, homes.clone());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(adopter.as_ref(), &stored, "test")
        .await
        .unwrap();
    let before = adopter.runtime_status(&handle).await.unwrap();
    assert_eq!(binding.boot_id.as_deref(), Some(before.boot_id.as_str()));
    assert!(!before.installation_ready);
    harness
        .backend
        .restart_persistent_process(&handle)
        .await
        .unwrap();
    assert!(adopter.runtime_inventory().await.unwrap()[0]
        .lifecycle
        .is_running());
    assert!(matches!(
        crate::sandbox::runtime_attachment::adopt_bound_runtime(adopter.as_ref(), &stored, "test")
            .await,
        Err(AdapterError::Unavailable(_))
    ));
    worker.run_one(&created.id, now).await.unwrap();
    let waiting = harness.stored(&created.id).await;
    assert_eq!(waiting.runtime, stored.runtime);
    assert_eq!(waiting.work.step, SandboxWorkStep::RunInstall);
    assert!(harness.actions.snapshot().is_empty());
    assert!(
        !harness
            .backend
            .snapshot(&handle)
            .await
            .unwrap()
            .credential_delivered
    );
}

#[tokio::test]
async fn control_plane_restart_adopts_exact_persistent_process_and_status_failure_preserves_authority(
) {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let homes = Homes::new(&harness);
    let worker = worker(&harness, homes.clone(), "original-worker");
    for _ in 0..8 {
        if harness.stored(&created.id).await.status == SandboxStatus::Active {
            break;
        }
        worker.run_one(&created.id, now).await.unwrap();
    }
    let mut active = harness.stored(&created.id).await;
    assert_eq!(active.status, SandboxStatus::Active);
    let revision = active.revision;
    active.runtime.as_mut().unwrap().runtime_version = runtime_image();
    active.revision += 1;
    harness
        .store
        .save_sandbox(revision, active.clone())
        .await
        .unwrap();
    let replacement = adapter(&harness, homes);
    let reconciler = crate::sandbox::reconciliation::SandboxReconciler::new(
        harness.store.clone(),
        replacement,
        Arc::new(harness.policy.clone()),
        "test".into(),
    );
    reconciler.initialize(now).await.unwrap();
    assert_eq!(harness.stored(&created.id).await, active);
    harness.backend.fail_next(
        WireOperation::RuntimeStatus,
        "observation_failed",
        "status could not be read",
    );
    assert!(reconciler.initialize(now).await.is_err());
    assert_eq!(harness.stored(&created.id).await, active);
    assert_eq!(harness.adapter.runtime_inventory().await.unwrap().len(), 1);
}

#[path = "home_routing_tests.rs"]
mod routing;
