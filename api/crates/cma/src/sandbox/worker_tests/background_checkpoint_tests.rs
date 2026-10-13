use super::*;
use crate::sandbox::checkpoint_maintenance::{ExportDependencies, SandboxCheckpointMaintenance};
use layer2_core::sandbox::store::AgentStore;
use layer2_core::sandbox::SandboxHome;
use layer3_adapter::hosted::{CheckpointArchiveStore, CheckpointArchiveStoreFactory};
use layer3_adapter::*;

#[derive(Default)]
struct SnapshotProvider {
    snapshots: Mutex<Vec<HomeSnapshotInventoryEntry>>,
    views: Mutex<Vec<HomeSnapshotViewInventoryEntry>>,
    jobs: Mutex<Vec<HomeSnapshotExportJobInventoryEntry>>,
    actions: Mutex<Vec<String>>,
    lose_snapshot_response: AtomicBool,
}

impl SnapshotProvider {
    fn record(&self, action: &str) {
        self.actions.lock().unwrap().push(action.into());
    }
}

#[async_trait::async_trait]
impl HomeSnapshotStore for SnapshotProvider {
    async fn snapshot_inventory(&self) -> layer3_adapter::Result<Vec<HomeSnapshotInventoryEntry>> {
        Ok(self.snapshots.lock().unwrap().clone())
    }
    async fn prepare_snapshot(
        &self,
        request: &HomeSnapshotRequest,
    ) -> layer3_adapter::Result<HomeSnapshotObservation> {
        self.record("allocate_snapshot");
        let receipt = HomeSnapshotReceipt {
            capture_id: request.capture_id.clone(),
            request_sha256: request.computed_sha256(),
            snapshot_reference: "snapshot".into(),
            snapshot_generation: "original-uid".into(),
        };
        self.snapshots
            .lock()
            .unwrap()
            .push(HomeSnapshotInventoryEntry {
                request: request.clone(),
                receipt: receipt.clone(),
            });
        if self.lose_snapshot_response.swap(false, Ordering::SeqCst) {
            return Err(AdapterError::Unavailable("lost response".into()));
        }
        Ok(HomeSnapshotObservation {
            receipt,
            captured_at: None,
            ready: false,
        })
    }
    async fn observe_snapshot(
        &self,
        request: &HomeSnapshotRequest,
        previous: &HomeSnapshotObservation,
    ) -> layer3_adapter::Result<HomeSnapshotObservation> {
        self.record("observe_snapshot");
        assert!(self
            .snapshots
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry.request == *request && entry.receipt == previous.receipt));
        Ok(HomeSnapshotObservation {
            receipt: previous.receipt.clone(),
            captured_at: Some("2026-09-05T00:30:01Z".into()),
            ready: true,
        })
    }
    async fn remove_snapshot(
        &self,
        request: &HomeSnapshotRequest,
        _: Option<&HomeSnapshotReceipt>,
    ) -> layer3_adapter::Result<()> {
        self.record(&format!("remove_snapshot:{}", request.epoch));
        self.snapshots
            .lock()
            .unwrap()
            .retain(|entry| entry.request != *request);
        Ok(())
    }
}

#[async_trait::async_trait]
impl HomeSnapshotViewStore for SnapshotProvider {
    async fn view_inventory(&self) -> layer3_adapter::Result<Vec<HomeSnapshotViewInventoryEntry>> {
        Ok(self.views.lock().unwrap().clone())
    }
    async fn prepare_view(
        &self,
        request: &HomeSnapshotExportRequest,
    ) -> layer3_adapter::Result<HomeSnapshotViewObservation> {
        self.record("allocate_view");
        let digest = request.computed_sha256()?;
        let receipt = HomeSnapshotViewReceipt {
            request_sha256: digest.clone(),
            volume_reference: format!("cma-view-{}", &digest[..52]),
            volume_generation: "view-uid".into(),
        };
        self.views
            .lock()
            .unwrap()
            .push(HomeSnapshotViewInventoryEntry {
                request: request.clone(),
                receipt: receipt.clone(),
            });
        Ok(HomeSnapshotViewObservation {
            receipt,
            bound_volume: None,
        })
    }
    async fn observe_view(
        &self,
        _: &HomeSnapshotExportRequest,
        _: &HomeSnapshotViewObservation,
    ) -> layer3_adapter::Result<HomeSnapshotViewObservation> {
        panic!("controller must schedule a consumer before awaiting disk binding")
    }
    async fn remove_view(
        &self,
        request: &HomeSnapshotExportRequest,
        _: Option<&HomeSnapshotViewReceipt>,
    ) -> layer3_adapter::Result<()> {
        self.record(&format!("remove_view:{}", request.snapshot.epoch));
        assert!(self
            .jobs
            .lock()
            .unwrap()
            .iter()
            .all(|job| job.request.export != *request));
        self.views
            .lock()
            .unwrap()
            .retain(|entry| entry.request != *request);
        Ok(())
    }
}

#[async_trait::async_trait]
impl HomeSnapshotExportJobStore for SnapshotProvider {
    async fn export_job_inventory(
        &self,
    ) -> layer3_adapter::Result<Vec<HomeSnapshotExportJobInventoryEntry>> {
        Ok(self.jobs.lock().unwrap().clone())
    }
    async fn prepare_export_job(
        &self,
        request: &HomeSnapshotExportJobRequest,
    ) -> layer3_adapter::Result<HomeSnapshotExportJobObservation> {
        self.record("allocate_job");
        let digest = request.computed_sha256()?;
        let receipt = HomeSnapshotExportJobReceipt {
            request_sha256: digest.clone(),
            pod_reference: format!("cma-export-{}", &digest[..48]),
            pod_generation: "pod-uid".into(),
        };
        self.jobs
            .lock()
            .unwrap()
            .push(HomeSnapshotExportJobInventoryEntry {
                request: request.clone(),
                receipt: receipt.clone(),
            });
        Ok(HomeSnapshotExportJobObservation {
            receipt,
            view: HomeSnapshotViewObservation {
                receipt: request.view.clone(),
                bound_volume: None,
            },
            phase: HomeSnapshotExportJobPhase::Pending,
        })
    }
    async fn observe_export_job(
        &self,
        _: &HomeSnapshotExportJobRequest,
        previous: &HomeSnapshotExportJobObservation,
    ) -> layer3_adapter::Result<HomeSnapshotExportJobObservation> {
        self.record("observe_job");
        let mut next = previous.clone();
        next.view.bound_volume = Some("derived-pv".into());
        next.phase = HomeSnapshotExportJobPhase::Ready;
        Ok(next)
    }
    async fn upload_export(
        &self,
        request: &HomeSnapshotExportJobRequest,
        _: &HomeSnapshotExportJobObservation,
        _: &dyn CheckpointArchiveStore,
    ) -> layer3_adapter::Result<HomeSnapshotExportReceipt> {
        self.record("upload");
        Ok(HomeSnapshotExportReceipt {
            request_sha256: request.export.computed_sha256()?,
            checkpoint: CheckpointReceipt {
                epoch: request.export.snapshot.epoch,
                object_reference: format!("home-{}.cma", "b".repeat(64)),
                sha256: "b".repeat(64),
                writers: WriterState::Resumed,
            },
            archive_bytes: 42,
            conversations: request
                .export
                .conversations
                .iter()
                .cloned()
                .map(|identity| HomeSnapshotExportConversation {
                    identity,
                    archive_sha256: "c".repeat(64),
                })
                .collect(),
        })
    }
    async fn remove_export_job(
        &self,
        request: &HomeSnapshotExportJobRequest,
        _: Option<&HomeSnapshotExportJobReceipt>,
    ) -> layer3_adapter::Result<()> {
        self.record(&format!("remove_job:{}", request.export.snapshot.epoch));
        self.jobs
            .lock()
            .unwrap()
            .retain(|entry| entry.request != *request);
        Ok(())
    }
}

struct UnusedArchiveFactory;
impl CheckpointArchiveStoreFactory for UnusedArchiveFactory {
    fn for_sandbox(&self, _: &str) -> layer3_adapter::Result<Arc<dyn CheckpointArchiveStore>> {
        Ok(Arc::new(UnusedArchiveFactory))
    }
}

#[async_trait::async_trait]
impl CheckpointArchiveStore for UnusedArchiveFactory {
    async fn upload(
        &self,
        _: layer3_adapter::hosted::CheckpointArchiveUpload,
    ) -> layer3_adapter::Result<layer3_adapter::hosted::CheckpointArchiveReceipt> {
        panic!("provider fixture supplies its own verified export receipt")
    }
    async fn download(
        &self,
        _: &str,
    ) -> layer3_adapter::Result<layer3_adapter::hosted::CheckpointArchiveDownload> {
        panic!("no archive reads in maintenance")
    }
}

fn worker(harness: &Harness, homes: &Arc<SnapshotProvider>) -> Arc<SandboxCheckpointMaintenance> {
    SandboxCheckpointMaintenance::with_export_dependencies(
        harness.store.clone(),
        harness.adapter.clone(),
        "test".into(),
        "export-worker".into(),
        Arc::new(harness.policy.clone()),
        ExportDependencies {
            homes: homes.clone(),
            objects: Arc::new(UnusedArchiveFactory),
            image: format!("registry/runtime@sha256:{}", "a".repeat(64)),
        },
    )
}

async fn fixture() -> (Harness, Sandbox, SystemTime, Arc<SnapshotProvider>) {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let mut active = harness.drive_active(&created.id, now).await;
    let mut home = SandboxHome::reserve(
        &active.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "cma-home-v1".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home-pvc".into(),
        storage_generation: "home-uid".into(),
    })
    .unwrap();
    active.lifecycle.home = Some(Box::new(home));
    active.revision += 1;
    harness
        .store
        .save_sandbox(active.revision - 1, active.clone())
        .await
        .unwrap();
    let mut agent = lifecycle_queue_tests::ready_agent(&active, now);
    agent.conversation.active_turn = Some(3);
    harness.store.create_agent(agent).await.unwrap();
    (
        harness,
        active,
        now + Duration::from_secs(1801),
        Arc::new(SnapshotProvider::default()),
    )
}

#[tokio::test]
async fn background_export_records_each_receipt_while_busy_and_schedules_consumer_before_binding() {
    let (harness, active, now, homes) = fixture().await;
    for step in 0..5 {
        // Recreate the worker every step to exercise durable resumption.
        worker(&harness, &homes)
            .run_once(now + Duration::from_secs(step))
            .await
            .unwrap();
        let current = harness.stored(&active.id).await;
        assert!(current.accepts_agent_and_workspace_admission());
        assert!(current.lifecycle.checkpoint_capture.is_none());
        assert_eq!(current.work, active.work);
    }
    let current = harness.stored(&active.id).await;
    let capture = current.lifecycle.background_checkpoint.as_ref().unwrap();
    assert_eq!(
        capture.progress.job.as_ref().unwrap().phase,
        HomeSnapshotExportJobPhase::Pending
    );
    assert!(capture
        .progress
        .view
        .as_ref()
        .unwrap()
        .bound_volume
        .is_none());
    assert_eq!(
        *homes.actions.lock().unwrap(),
        [
            "allocate_snapshot",
            "observe_snapshot",
            "allocate_view",
            "allocate_job"
        ]
    );
    assert!(harness
        .backend
        .requests()
        .await
        .iter()
        .all(|(_, request)| !matches!(
            request,
            RequestFrame::Checkpoint(_) | RequestFrame::CheckpointForTeardown(_)
        )));
}

#[tokio::test]
async fn background_export_lost_allocation_recovers_same_uid_or_aborts_without_recapture() {
    for remove in [false, true] {
        let (harness, active, now, homes) = fixture().await;
        homes.lose_snapshot_response.store(true, Ordering::SeqCst);
        worker(&harness, &homes).run_once(now).await.unwrap();
        assert!(worker(&harness, &homes)
            .run_once(now + Duration::from_secs(1))
            .await
            .is_err());
        if remove {
            homes.snapshots.lock().unwrap().clear();
        }
        worker(&harness, &homes)
            .run_once(now + Duration::from_secs(32))
            .await
            .unwrap();
        let current = harness.stored(&active.id).await;
        let progress = &current
            .lifecycle
            .background_checkpoint
            .as_ref()
            .unwrap()
            .progress;
        assert_eq!(progress.cancelled, remove);
        assert_eq!(
            progress
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.receipt.snapshot_generation.as_str()),
            (!remove).then_some("original-uid")
        );
        assert_eq!(
            homes
                .actions
                .lock()
                .unwrap()
                .iter()
                .filter(|action| *action == "allocate_snapshot")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn background_export_pause_cancels_and_removes_consumer_before_derived_disk() {
    let (harness, active, now, homes) = fixture().await;
    for step in 0..5 {
        worker(&harness, &homes)
            .run_once(now + Duration::from_secs(step))
            .await
            .unwrap();
    }
    let current = harness.stored(&active.id).await;
    let mut pausing = current.clone();
    pausing
        .begin_pause(rfc3339_at(now + Duration::from_secs(5)), None)
        .unwrap();
    harness
        .store
        .save_sandbox(current.revision, pausing)
        .await
        .unwrap();
    worker(&harness, &homes)
        .run_once(now + Duration::from_secs(6))
        .await
        .unwrap();
    worker(&harness, &homes)
        .run_once(now + Duration::from_secs(7))
        .await
        .unwrap();
    let current = harness.stored(&active.id).await;
    assert_eq!(current.status, SandboxStatus::Pausing);
    assert!(current.lifecycle.background_checkpoint.is_none());
    let actions = homes.actions.lock().unwrap();
    assert_eq!(
        &actions[4..],
        ["remove_job:1", "remove_view:1", "remove_snapshot:1"]
    );
    assert_eq!(current.next_checkpoint_epoch().unwrap(), 2);
}

#[tokio::test]
async fn background_export_cleanup_preserves_newer_epochs_and_only_latest_completed_snapshot() {
    let (harness, active, now, homes) = fixture().await;
    for step in 0..9 {
        worker(&harness, &homes)
            .run_once(now + Duration::from_secs(step))
            .await
            .unwrap();
    }
    let first = harness.stored(&active.id).await;
    assert!(first.lifecycle.background_checkpoint.is_none());
    assert_eq!(
        first.lifecycle.checkpoints.latest.as_ref().unwrap().epoch,
        1
    );
    let later = now + Duration::from_secs(1801);
    for step in 0..8 {
        worker(&harness, &homes)
            .run_once(later + Duration::from_secs(step))
            .await
            .unwrap();
    }
    let published = harness.stored(&active.id).await;
    assert!(
        published
            .lifecycle
            .background_checkpoint
            .as_ref()
            .unwrap()
            .progress
            .published
    );
    // A newer physical capture can exist while an older cleanup is running.
    let mut future = homes.snapshots.lock().unwrap().last().unwrap().clone();
    future.request.epoch = 3;
    future.request.capture_id = format!("{}-checkpoint-3", active.id);
    future.receipt.capture_id = future.request.capture_id.clone();
    future.receipt.request_sha256 = future.request.computed_sha256();
    homes.snapshots.lock().unwrap().push(future.clone());
    worker(&harness, &homes)
        .run_once(later + Duration::from_secs(8))
        .await
        .unwrap();
    let current = harness.stored(&active.id).await;
    assert!(current.lifecycle.background_checkpoint.is_none());
    assert_eq!(
        current.lifecycle.checkpoints.latest.as_ref().unwrap().epoch,
        2
    );
    assert!(current.lifecycle.checkpoints.previous.is_none());
    let snapshots = homes.snapshots.lock().unwrap();
    assert_eq!(
        snapshots
            .iter()
            .map(|entry| entry.request.epoch)
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert_eq!(snapshots[1], future);
    assert!(homes.views.lock().unwrap().is_empty());
    assert!(homes.jobs.lock().unwrap().is_empty());
}
