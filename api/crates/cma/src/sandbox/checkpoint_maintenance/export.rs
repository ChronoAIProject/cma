//! Each provider allocation/observation is persisted before the next side effect.

use std::time::Duration;

use layer2_core::sandbox::store::{SandboxCheckpointMutation, SandboxMutationGuard};
use layer2_core::sandbox::{Sandbox, SandboxBackgroundCheckpointPlan, SandboxCheckpointRef};
use layer3_adapter::{HomeSnapshotExportJobPhase, HomeSnapshotRequest};

use super::{model_error, CaptureClock, SandboxCheckpointMaintenance, SandboxStoreError};

const PROVIDER_POLL: Duration = Duration::from_secs(5);
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(4 * 60);
const EXPORT_DEADLINE_SECONDS: u64 = 15 * 60;

impl SandboxCheckpointMaintenance {
    pub(super) async fn begin_export(
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
        let epoch = sandbox.next_checkpoint_epoch().map_err(model_error)?;
        let mut conversations = self
            .agents(sandbox)
            .await?
            .into_iter()
            .filter(|agent| agent.status.is_live())
            .filter_map(|agent| {
                agent.conversation.conversation_id.map(|conversation_id| {
                    layer3_adapter::HomeSnapshotConversationIdentity {
                        agent_id: agent.id.to_string(),
                        conversation_id,
                    }
                })
            })
            .collect::<Vec<_>>();
        conversations.sort();
        let plan = SandboxBackgroundCheckpointPlan {
            snapshot: HomeSnapshotRequest {
                capture_id: format!("{}-checkpoint-{}", sandbox.id, epoch),
                epoch,
                home,
                snapshot_class: "cma-home-v1".into(),
            },
            runtime: sandbox.runtime.clone().ok_or(SandboxStoreError::Conflict)?,
            conversations,
            image: dependencies.image.clone(),
        };
        let mut next = sandbox.clone();
        let stamp = layer2_core::rfc3339_at(clock.now());
        next.begin_background_checkpoint(plan, stamp.clone())
            .map_err(model_error)?;
        self.commit(sandbox, next, stamp).await
    }

    pub(super) async fn process_export(
        &self,
        sandbox: &Sandbox,
        clock: &CaptureClock,
    ) -> Result<bool, SandboxStoreError> {
        let capture = sandbox
            .lifecycle
            .background_checkpoint
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let now = clock.now();
        let stamp = layer2_core::rfc3339_at(now);
        if !capture.work.is_due(&stamp) {
            return Ok(false);
        }
        let mut claimed = sandbox.clone();
        claimed
            .claim_background_checkpoint(
                self.holder.clone(),
                stamp.clone(),
                layer2_core::rfc3339_at(now + super::CLAIM_DURATION),
            )
            .map_err(model_error)?;
        self.commit(sandbox, claimed.clone(), stamp).await?;
        let result =
            tokio::time::timeout(PROVIDER_TIMEOUT, self.export_step(&claimed, clock)).await;
        match result {
            Ok(Ok(())) => Ok(true),
            result => {
                // Release a bounded failed step; retain every recorded receipt.
                let current = self.current_export_claim(&claimed).await?;
                let capture = current
                    .lifecycle
                    .background_checkpoint
                    .as_ref()
                    .ok_or(SandboxStoreError::Conflict)?;
                let lease = capture
                    .work
                    .lease
                    .as_ref()
                    .ok_or(SandboxStoreError::Conflict)?;
                let mut next = current.clone();
                let now = clock.now();
                let stamp = layer2_core::rfc3339_at(now);
                next.advance_background_checkpoint(
                    capture.progress.clone(),
                    &lease.holder,
                    lease.fence,
                    stamp.clone(),
                    layer2_core::rfc3339_at(now + super::RETRY_DELAY),
                )
                .map_err(model_error)?;
                self.commit(&current, next, stamp).await?;
                match result {
                    Ok(Err(error)) => Err(error),
                    _ => Err(SandboxStoreError::Unavailable(
                        "snapshot provider step exceeded its deadline".into(),
                    )),
                }
            }
        }
    }

    async fn export_step(
        &self,
        claimed: &Sandbox,
        clock: &CaptureClock,
    ) -> Result<(), SandboxStoreError> {
        let dependencies = self.export.as_ref().ok_or(SandboxStoreError::Conflict)?;
        let capture = claimed
            .lifecycle
            .background_checkpoint
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let mut progress = capture.progress.clone();
        let started = layer2_core::seconds_from_rfc3339(&capture.started_at)
            .ok_or(SandboxStoreError::Conflict)?;
        let now = layer2_core::seconds_from_rfc3339(&layer2_core::rfc3339_at(clock.now()))
            .ok_or(SandboxStoreError::Conflict)?;
        let expired = now.saturating_sub(started) >= EXPORT_DEADLINE_SECONDS;
        if progress.published || progress.cancelled {
            self.cleanup_export(claimed).await?;
            let current = self.current_export_claim(claimed).await?;
            let lease = current
                .lifecycle
                .background_checkpoint
                .as_ref()
                .and_then(|capture| capture.work.lease.as_ref())
                .ok_or(SandboxStoreError::Conflict)?;
            let stamp = layer2_core::rfc3339_at(clock.now());
            let mut next = current.clone();
            next.complete_background_checkpoint_cleanup(&lease.holder, lease.fence, stamp.clone())
                .map_err(model_error)?;
            return self.commit(&current, next, stamp).await;
        } else if expired || !capture.source_is_current(claimed) {
            progress.cancelled = true;
        } else if let Some(receipt) = &progress.uploaded {
            let checkpoint = SandboxCheckpointRef::seal_export(
                &capture.export_request().map_err(model_error)?,
                receipt,
            )
            .map_err(model_error)?;
            let current = self.current_export_claim(claimed).await?;
            let lease = current
                .lifecycle
                .background_checkpoint
                .as_ref()
                .and_then(|capture| capture.work.lease.as_ref())
                .ok_or(SandboxStoreError::Conflict)?;
            let stamp = layer2_core::rfc3339_at(clock.now());
            let mut next = current.clone();
            next.publish_background_checkpoint(
                checkpoint.clone(),
                &lease.holder,
                lease.fence,
                stamp.clone(),
            )
            .map_err(model_error)?;
            return self
                .store
                .publish_sandbox_checkpoint(SandboxCheckpointMutation {
                    guard: SandboxMutationGuard::capture(&current),
                    agents: Vec::new(),
                    nonblocking: true,
                    checkpoint,
                    sandbox: next,
                    appended_at: stamp,
                })
                .await;
        } else if let Some(job) = &progress.job {
            let request = capture.job_request().map_err(model_error)?;
            match job.phase {
                HomeSnapshotExportJobPhase::Failed => progress.cancelled = true,
                HomeSnapshotExportJobPhase::Ready => {
                    let archive = dependencies
                        .objects
                        .for_sandbox(claimed.id.as_str())
                        .map_err(provider_error)?;
                    progress.uploaded = Some(
                        dependencies
                            .homes
                            .upload_export(&request, job, archive.as_ref())
                            .await
                            .map_err(provider_error)?,
                    );
                }
                _ => {
                    progress.job = Some(
                        dependencies
                            .homes
                            .observe_export_job(&request, job)
                            .await
                            .map_err(provider_error)?,
                    )
                }
            }
        } else if progress.view.is_some() {
            // WaitForFirstConsumer volumes need this Pod before they can bind.
            progress.job = Some(
                dependencies
                    .homes
                    .prepare_export_job(&capture.job_request().map_err(model_error)?)
                    .await
                    .map_err(provider_error)?,
            );
        } else if progress
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.ready)
        {
            progress.view = Some(
                dependencies
                    .homes
                    .prepare_view(&capture.export_request().map_err(model_error)?)
                    .await
                    .map_err(provider_error)?,
            );
        } else if let Some(snapshot) = &progress.snapshot {
            progress.snapshot = Some(
                dependencies
                    .homes
                    .observe_snapshot(&capture.plan.snapshot, snapshot)
                    .await
                    .map_err(provider_error)?,
            );
        } else if capture
            .work
            .lease
            .as_ref()
            .is_some_and(|lease| lease.fence == 1)
        {
            progress.snapshot = Some(
                dependencies
                    .homes
                    .prepare_snapshot(&capture.plan.snapshot)
                    .await
                    .map_err(provider_error)?,
            );
        } else {
            // An abandoned first claim may already have allocated its disk
            // snapshot. Recover that exact receipt; an absent resource aborts
            // this epoch, so a restart can never move its capture boundary.
            let existing = dependencies
                .homes
                .snapshot_inventory()
                .await
                .map_err(provider_error)?
                .into_iter()
                .find(|entry| entry.request == capture.plan.snapshot);
            if let Some(entry) = existing {
                progress.snapshot = Some(layer3_adapter::HomeSnapshotObservation {
                    receipt: entry.receipt,
                    captured_at: None,
                    ready: false,
                });
            } else {
                progress.cancelled = true;
            }
        }
        let current = self.current_export_claim(claimed).await?;
        let lease = current
            .lifecycle
            .background_checkpoint
            .as_ref()
            .and_then(|capture| capture.work.lease.as_ref())
            .ok_or(SandboxStoreError::Conflict)?;
        let now = clock.now();
        let stamp = layer2_core::rfc3339_at(now);
        let delay = if progress == capture.progress {
            PROVIDER_POLL
        } else {
            Duration::ZERO
        };
        let mut next = current.clone();
        next.advance_background_checkpoint(
            progress,
            &lease.holder,
            lease.fence,
            stamp.clone(),
            layer2_core::rfc3339_at(now + delay),
        )
        .map_err(model_error)?;
        self.commit(&current, next, stamp).await
    }

    async fn current_export_claim(&self, claimed: &Sandbox) -> Result<Sandbox, SandboxStoreError> {
        let current = self
            .store
            .get_sandbox(&claimed.id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if current.lifecycle.background_checkpoint != claimed.lifecycle.background_checkpoint {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(current)
    }

    async fn cleanup_export(&self, claimed: &Sandbox) -> Result<(), SandboxStoreError> {
        let dependencies = self.export.as_ref().ok_or(SandboxStoreError::Conflict)?;
        let capture = claimed
            .lifecycle
            .background_checkpoint
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        if capture.progress.view.is_some() {
            dependencies
                .homes
                .remove_export_job(
                    &capture.job_request().map_err(model_error)?,
                    capture.progress.job.as_ref().map(|job| &job.receipt),
                )
                .await
                .map_err(provider_error)?;
        }
        if capture
            .progress
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.ready)
        {
            dependencies
                .homes
                .remove_view(
                    &capture.export_request().map_err(model_error)?,
                    capture.progress.view.as_ref().map(|view| &view.receipt),
                )
                .await
                .map_err(provider_error)?;
        }
        if capture.progress.cancelled {
            dependencies
                .homes
                .remove_snapshot(
                    &capture.plan.snapshot,
                    capture
                        .progress
                        .snapshot
                        .as_ref()
                        .map(|snapshot| &snapshot.receipt),
                )
                .await
                .map_err(provider_error)?;
        }
        let current = self.current_export_claim(claimed).await?;
        if let Some(latest) = &current.lifecycle.checkpoints.latest {
            // Strictly older epochs only; retire consumers before their disks.
            for entry in dependencies
                .homes
                .export_job_inventory()
                .await
                .map_err(provider_error)?
            {
                if entry.request.export.snapshot.home.request.sandbox_id == claimed.id.as_str()
                    && entry.request.export.snapshot.epoch < latest.epoch
                {
                    dependencies
                        .homes
                        .remove_export_job(&entry.request, Some(&entry.receipt))
                        .await
                        .map_err(provider_error)?;
                }
            }
            for entry in dependencies
                .homes
                .view_inventory()
                .await
                .map_err(provider_error)?
            {
                if entry.request.snapshot.home.request.sandbox_id == claimed.id.as_str()
                    && entry.request.snapshot.epoch < latest.epoch
                {
                    dependencies
                        .homes
                        .remove_view(&entry.request, Some(&entry.receipt))
                        .await
                        .map_err(provider_error)?;
                }
            }
            for entry in dependencies
                .homes
                .snapshot_inventory()
                .await
                .map_err(provider_error)?
            {
                if entry.request.home.request.sandbox_id == claimed.id.as_str()
                    && entry.request.epoch < latest.epoch
                {
                    dependencies
                        .homes
                        .remove_snapshot(&entry.request, Some(&entry.receipt))
                        .await
                        .map_err(provider_error)?;
                }
            }
        }
        Ok(())
    }
}

fn provider_error(error: layer3_adapter::AdapterError) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
