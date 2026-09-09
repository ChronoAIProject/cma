//! Nonblocking persistent-home checkpoint publication.
//!
//! The provider snapshot is taken while the live runtime remains writable. The
//! export job then reads a separate CSI-derived view, so user actions continue
//! while the archive is being produced. Provider allocations are deterministic
//! and durable; a controller restart can repeat this method and observe the
//! same snapshot/view/job records.

use std::time::Duration;

use layer2_core::sandbox::store::{SandboxCheckpointMutation, SandboxMutationGuard};
use layer2_core::sandbox::{Sandbox, SandboxCheckpointRef};
use layer3_adapter::{
    HomeSnapshotExportJobRequest, HomeSnapshotExportJobStore, HomeSnapshotExportRequest,
    HomeSnapshotRequest, HomeSnapshotStore, HomeSnapshotViewStore,
};

use super::SandboxStoreError;
use super::{model_error, CaptureClock, SandboxCheckpointMaintenance};

const PROVIDER_POLL: Duration = Duration::from_secs(2);
const EXPORT_DEADLINE: Duration = Duration::from_secs(15 * 60);

impl SandboxCheckpointMaintenance {
    pub(super) async fn capture_and_publish_export(
        &self,
        sandbox: &Sandbox,
        clock: &CaptureClock,
    ) -> Result<(), SandboxStoreError> {
        let dependencies = self.export.as_ref().ok_or(SandboxStoreError::Conflict)?;
        let home = sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .ok_or(SandboxStoreError::Conflict)?;
        let epoch = sandbox
            .lifecycle
            .checkpoints
            .latest
            .as_ref()
            .map_or(1, |checkpoint| checkpoint.epoch.saturating_add(1));
        if epoch == 0 {
            return Err(SandboxStoreError::Conflict);
        }
        let agents = self.agents(sandbox).await?;
        let mut conversations = agents
            .iter()
            .filter(|agent| agent.status.is_live())
            .filter_map(|agent| {
                agent
                    .conversation
                    .conversation_id
                    .clone()
                    .map(
                        |conversation_id| layer3_adapter::HomeSnapshotConversationIdentity {
                            agent_id: agent.id.to_string(),
                            conversation_id,
                        },
                    )
            })
            .collect::<Vec<_>>();
        conversations.sort();
        let snapshot = HomeSnapshotRequest {
            capture_id: format!("{}-checkpoint-{}", sandbox.id, epoch),
            epoch,
            home,
            snapshot_class: "cma-home-v1".into(),
        };
        let result = tokio::time::timeout(EXPORT_DEADLINE, async {
            let mut observation = dependencies
                .homes
                .prepare_snapshot(&snapshot)
                .await
                .map_err(provider_error)?;
            while !observation.ready {
                tokio::time::sleep(PROVIDER_POLL).await;
                observation = dependencies
                    .homes
                    .observe_snapshot(&snapshot, &observation)
                    .await
                    .map_err(provider_error)?;
            }
            let request = HomeSnapshotExportRequest {
                snapshot,
                observation,
                runtime_generation: sandbox
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.provision_generation)
                    .unwrap_or(1),
                conversations,
            };
            // A controller restart may rediscover the same capture after Agent
            // rows have advanced. Retire only older derived attempts for this
            // deterministic capture identity before creating the new view/job.
            for entry in dependencies
                .homes
                .view_inventory()
                .await
                .map_err(provider_error)?
            {
                if entry.request.snapshot.capture_id == request.snapshot.capture_id
                    && entry.request != request
                {
                    dependencies
                        .homes
                        .remove_view(&entry.request, Some(&entry.receipt))
                        .await
                        .map_err(provider_error)?;
                }
            }
            let mut view = dependencies
                .homes
                .prepare_view(&request)
                .await
                .map_err(provider_error)?;
            while view.bound_volume.is_none() {
                tokio::time::sleep(PROVIDER_POLL).await;
                view = dependencies
                    .homes
                    .observe_view(&request, &view)
                    .await
                    .map_err(provider_error)?;
            }
            let job = HomeSnapshotExportJobRequest {
                export: request,
                view: view.receipt.clone(),
                attempt: 1,
                image: dependencies.image.clone(),
            };
            for entry in dependencies
                .homes
                .export_job_inventory()
                .await
                .map_err(provider_error)?
            {
                if entry.request.export.snapshot.capture_id == job.export.snapshot.capture_id
                    && entry.request != job
                {
                    dependencies
                        .homes
                        .remove_export_job(&entry.request, Some(&entry.receipt))
                        .await
                        .map_err(provider_error)?;
                }
            }
            let mut observed = dependencies
                .homes
                .prepare_export_job(&job)
                .await
                .map_err(provider_error)?;
            while matches!(
                observed.phase,
                layer3_adapter::HomeSnapshotExportJobPhase::Pending
                    | layer3_adapter::HomeSnapshotExportJobPhase::Exporting
            ) {
                tokio::time::sleep(PROVIDER_POLL).await;
                observed = dependencies
                    .homes
                    .observe_export_job(&job, &observed)
                    .await
                    .map_err(provider_error)?;
            }
            if observed.phase != layer3_adapter::HomeSnapshotExportJobPhase::Ready {
                return Err(SandboxStoreError::Unavailable(
                    "snapshot export job failed".into(),
                ));
            }
            let archive = dependencies
                .objects
                .for_sandbox(sandbox.id.as_str())
                .map_err(provider_error)?;
            let receipt = dependencies
                .homes
                .upload_export(&job, &observed, archive.as_ref())
                .await
                .map_err(provider_error)?;
            let checkpoint =
                SandboxCheckpointRef::seal_export(&job.export, &receipt).map_err(model_error)?;
            let current = self
                .store
                .get_sandbox(&sandbox.id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?;
            self.store
                .publish_sandbox_checkpoint(SandboxCheckpointMutation {
                    guard: SandboxMutationGuard::capture(&current),
                    agents: Vec::new(),
                    nonblocking: true,
                    checkpoint,
                    sandbox: current,
                    appended_at: layer2_core::rfc3339_at(clock.now()),
                })
                .await?;
            // Keep one physical recovery point per Sandbox.  The newly
            // published capture is now authoritative, so older captures (and
            // any abandoned retry from a prior epoch) can be released with
            // their exact provider receipts.  Cleanup is receipt guarded by
            // the provider and is safe to repeat after a controller restart.
            for entry in dependencies
                .homes
                .snapshot_inventory()
                .await
                .map_err(provider_error)?
            {
                if entry.request.home.request.sandbox_id
                    == job.export.snapshot.home.request.sandbox_id
                    && entry.request.capture_id != job.export.snapshot.capture_id
                {
                    dependencies
                        .homes
                        .remove_snapshot(&entry.request, Some(&entry.receipt))
                        .await
                        .map_err(provider_error)?;
                }
            }
            // The immutable source snapshot remains as the Sandbox's latest
            // physical recovery point. Export Pods and derived writable views
            // are staging resources and can be retired after publication.
            dependencies
                .homes
                .remove_export_job(&job, Some(&observed.receipt))
                .await
                .map_err(provider_error)?;
            dependencies
                .homes
                .remove_view(&job.export, Some(&job.view))
                .await
                .map_err(provider_error)?;
            Ok::<(), SandboxStoreError>(())
        })
        .await;
        match result {
            Ok(result) => result,
            Err(_) => Err(SandboxStoreError::Unavailable(
                "snapshot export exceeded its bounded deadline".into(),
            )),
        }
    }
}

fn provider_error(error: layer3_adapter::AdapterError) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
