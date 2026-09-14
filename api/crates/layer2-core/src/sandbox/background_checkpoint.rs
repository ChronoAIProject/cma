//! Restartable exports from an immutable disk without holding live writers.

use layer3_adapter::{
    HomeSnapshotConversationIdentity, HomeSnapshotExportJobObservation, HomeSnapshotExportJobPhase,
    HomeSnapshotExportJobRequest, HomeSnapshotExportReceipt, HomeSnapshotExportRequest,
    HomeSnapshotObservation, HomeSnapshotRequest, HomeSnapshotViewObservation,
};
use serde::{Deserialize, Serialize};

use super::{
    RuntimeBinding, Sandbox, SandboxCheckpointRef, SandboxModelError, SandboxStatus,
    SandboxWorkState, SandboxWorkStep,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxBackgroundCheckpointPlan {
    pub snapshot: HomeSnapshotRequest,
    pub runtime: RuntimeBinding,
    pub conversations: Vec<HomeSnapshotConversationIdentity>,
    pub image: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxBackgroundCheckpointProgress {
    pub snapshot: Option<HomeSnapshotObservation>,
    pub view: Option<HomeSnapshotViewObservation>,
    pub job: Option<HomeSnapshotExportJobObservation>,
    pub uploaded: Option<HomeSnapshotExportReceipt>,
    pub published: bool,
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxBackgroundCheckpoint {
    pub plan: SandboxBackgroundCheckpointPlan,
    pub progress: SandboxBackgroundCheckpointProgress,
    pub started_at: String,
    pub work: SandboxWorkState,
}

impl SandboxBackgroundCheckpoint {
    pub fn public_phase(&self) -> &'static str {
        let progress = &self.progress;
        if progress.cancelled {
            "cancelled"
        } else if progress.published {
            "saved"
        } else if progress.uploaded.is_some() {
            "finishing"
        } else if progress
            .job
            .as_ref()
            .is_some_and(|job| job.phase == HomeSnapshotExportJobPhase::Ready)
        {
            "uploading"
        } else if progress.job.is_some() {
            "exporting"
        } else if progress
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.ready)
        {
            "preparing_export"
        } else {
            "capturing"
        }
    }
    pub fn export_request(&self) -> Result<HomeSnapshotExportRequest, SandboxModelError> {
        let request = HomeSnapshotExportRequest {
            snapshot: self.plan.snapshot.clone(),
            observation: self.progress.snapshot.clone().ok_or_else(invalid)?,
            runtime_generation: self.plan.runtime.provision_generation.ok_or_else(invalid)?,
            conversations: self.plan.conversations.clone(),
        };
        request.validate().map_err(|_| invalid())?;
        Ok(request)
    }

    pub fn job_request(&self) -> Result<HomeSnapshotExportJobRequest, SandboxModelError> {
        let request = HomeSnapshotExportJobRequest {
            export: self.export_request()?,
            view: self
                .progress
                .view
                .as_ref()
                .ok_or_else(invalid)?
                .receipt
                .clone(),
            attempt: 1,
            image: self.plan.image.clone(),
        };
        request.validate().map_err(|_| invalid())?;
        Ok(request)
    }

    pub fn source_is_current(&self, sandbox: &Sandbox) -> bool {
        sandbox.status == SandboxStatus::Active
            && sandbox.lifecycle.operation.is_none()
            && sandbox.lifecycle.checkpoint_capture.is_none()
            && sandbox.runtime.as_ref() == Some(&self.plan.runtime)
            && sandbox
                .lifecycle
                .home
                .as_ref()
                .and_then(|home| home.attachment())
                == Some(self.plan.snapshot.home.clone())
            && sandbox
                .lifecycle
                .checkpoints
                .latest
                .as_ref()
                .is_none_or(|latest| latest.epoch < self.plan.snapshot.epoch)
    }

    pub(super) fn validate(&self, sandbox: &Sandbox) -> Result<(), SandboxModelError> {
        self.plan.snapshot.validate().map_err(|_| invalid())?;
        if self.plan.snapshot.home.request.sandbox_id != sandbox.id.as_str()
            || self.plan.snapshot.epoch > sandbox.lifecycle.checkpoint_epoch_high_water
            || self.plan.snapshot.capture_id
                != format!("{}-checkpoint-{}", sandbox.id, self.plan.snapshot.epoch)
            || crate::seconds_from_rfc3339(&self.started_at).is_none()
            || self.work.step != SandboxWorkStep::RunLifecycleOperation
            || self.work.execution.is_some()
            || self.work.next_fence == 0
            || crate::seconds_from_rfc3339(&self.work.ready_at).is_none()
            || self.work.lease.as_ref().is_some_and(|lease| {
                lease.holder.trim().is_empty()
                    || lease.fence == 0
                    || lease.fence >= self.work.next_fence
                    || crate::seconds_from_rfc3339(&lease.expires_at).is_none()
            })
            || (self.progress.published && self.progress.cancelled)
        {
            return Err(invalid());
        }
        if self
            .plan
            .runtime
            .provision_generation
            .is_none_or(|generation| generation == 0)
        {
            return Err(invalid());
        }
        HomeSnapshotExportRequest::validate_conversations(&self.plan.conversations)
            .map_err(|_| invalid())?;
        HomeSnapshotExportJobRequest::validate_image(&self.plan.image).map_err(|_| invalid())?;
        if let Some(snapshot) = &self.progress.snapshot {
            snapshot
                .receipt
                .validate_for(&self.plan.snapshot)
                .map_err(|_| invalid())?;
            if snapshot.ready {
                self.export_request()?;
            }
        }
        if let Some(view) = &self.progress.view {
            view.receipt
                .validate_for(&self.export_request()?)
                .map_err(|_| invalid())?;
        }
        if let Some(job) = &self.progress.job {
            let request = self.job_request()?;
            job.receipt.validate_for(&request).map_err(|_| invalid())?;
            job.view
                .receipt
                .validate_for(&request.export)
                .map_err(|_| invalid())?;
            if job.view.receipt != request.view
                || (job.phase == HomeSnapshotExportJobPhase::Ready
                    && job.view.bound_volume.is_none())
            {
                return Err(invalid());
            }
        }
        if let Some(uploaded) = &self.progress.uploaded {
            if self
                .progress
                .job
                .as_ref()
                .is_none_or(|job| job.phase != HomeSnapshotExportJobPhase::Ready)
            {
                return Err(invalid());
            }
            uploaded
                .validate_for(&self.export_request()?)
                .map_err(|_| invalid())?;
        }
        if self.progress.published && self.progress.uploaded.is_none() {
            return Err(invalid());
        }
        Ok(())
    }
}

impl Sandbox {
    pub fn begin_background_checkpoint(
        &mut self,
        plan: SandboxBackgroundCheckpointPlan,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.validate()?;
        if self.lifecycle.background_checkpoint.is_some()
            || self.work.lease.is_some()
            || plan.snapshot.epoch != self.next_checkpoint_epoch()?
        {
            return Err(invalid());
        }
        let capture = SandboxBackgroundCheckpoint {
            plan,
            progress: SandboxBackgroundCheckpointProgress::default(),
            started_at: at.clone(),
            work: SandboxWorkState::new(SandboxWorkStep::RunLifecycleOperation, at.clone()),
        };
        if !capture.source_is_current(self) {
            return Err(invalid());
        }
        let mut next = self.clone();
        next.lifecycle.checkpoint_epoch_high_water = capture.plan.snapshot.epoch;
        next.lifecycle.background_checkpoint = Some(Box::new(capture));
        self.publish_background_change(next, at)
    }

    pub fn claim_background_checkpoint(
        &mut self,
        holder: String,
        at: String,
        expires_at: String,
    ) -> Result<(), SandboxModelError> {
        let mut next = self.clone();
        next.lifecycle
            .background_checkpoint
            .as_mut()
            .ok_or_else(invalid)?
            .work
            .claim(holder, &at, expires_at)?;
        self.publish_background_change(next, at)
    }

    /// One provider result at a time; recorded allocation identities never change.
    pub fn advance_background_checkpoint(
        &mut self,
        progress: SandboxBackgroundCheckpointProgress,
        holder: &str,
        fence: u64,
        at: String,
        ready_at: String,
    ) -> Result<(), SandboxModelError> {
        let capture = self.require_background_claim(holder, fence, &at)?;
        if ready_at < at || !progress_successor(&capture.progress, &progress) {
            return Err(invalid());
        }
        let mut next = self.clone();
        let capture = next
            .lifecycle
            .background_checkpoint
            .as_mut()
            .ok_or_else(invalid)?;
        capture.progress = progress;
        capture.work.retry(holder, fence, ready_at)?;
        self.publish_background_change(next, at)
    }

    pub fn publish_background_checkpoint(
        &mut self,
        checkpoint: SandboxCheckpointRef,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        let capture = self.require_background_claim(holder, fence, &at)?;
        if !capture.source_is_current(self)
            || self.work.lease.is_some()
            || capture.progress.cancelled
            || capture.progress.published
        {
            return Err(invalid());
        }
        checkpoint.validate_export(
            &capture.export_request()?,
            capture.progress.uploaded.as_ref().ok_or_else(invalid)?,
        )?;
        let mut next = self.clone();
        next.lifecycle.checkpoints.publish(checkpoint)?;
        let capture = next
            .lifecycle
            .background_checkpoint
            .as_mut()
            .ok_or_else(invalid)?;
        capture.progress.published = true;
        capture.work.retry(holder, fence, at.clone())?;
        self.publish_background_change(next, at)
    }

    pub fn complete_background_checkpoint_cleanup(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        let capture = self.require_background_claim(holder, fence, &at)?;
        if !capture.progress.published && !capture.progress.cancelled {
            return Err(invalid());
        }
        let mut next = self.clone();
        next.lifecycle.background_checkpoint = None;
        self.publish_background_change(next, at)
    }

    fn require_background_claim(
        &self,
        holder: &str,
        fence: u64,
        at: &str,
    ) -> Result<&SandboxBackgroundCheckpoint, SandboxModelError> {
        self.validate()?;
        let capture = self
            .lifecycle
            .background_checkpoint
            .as_deref()
            .ok_or_else(invalid)?;
        if !capture.work.owns(holder, fence)
            || capture
                .work
                .lease
                .as_ref()
                .is_none_or(|lease| lease.expires_at.as_str() <= at)
        {
            return Err(invalid());
        }
        Ok(capture)
    }

    fn publish_background_change(
        &mut self,
        mut next: Self,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if crate::seconds_from_rfc3339(&at).is_none() || at < self.updated_at {
            return Err(invalid());
        }
        next.revision = self.revision.checked_add(1).ok_or_else(invalid)?;
        next.updated_at = at;
        next.validate()?;
        *self = next;
        Ok(())
    }
}

fn progress_successor(
    old: &SandboxBackgroundCheckpointProgress,
    next: &SandboxBackgroundCheckpointProgress,
) -> bool {
    if old == next {
        return true;
    }
    if old.published || old.cancelled || next.published {
        return false;
    }
    let mut expected = old.clone();
    if next.cancelled {
        expected.cancelled = true;
    } else if old.snapshot != next.snapshot {
        let Some(snapshot) = &next.snapshot else {
            return false;
        };
        if old.snapshot.as_ref().is_some_and(|prior| {
            prior.ready
                || prior.receipt != snapshot.receipt
                || (prior.captured_at.is_some() && prior.captured_at != snapshot.captured_at)
        }) {
            return false;
        }
        expected.snapshot = next.snapshot.clone();
    } else if old.view != next.view {
        if old.view.is_some() || next.view.is_none() {
            return false;
        }
        expected.view = next.view.clone();
    } else if old.job != next.job {
        let Some(job) = &next.job else {
            return false;
        };
        if old.job.as_ref().is_some_and(|prior| {
            prior.receipt != job.receipt
                || prior.view.receipt != job.view.receipt
                || (prior.view.bound_volume.is_some()
                    && prior.view.bound_volume != job.view.bound_volume)
                || match prior.phase {
                    HomeSnapshotExportJobPhase::Ready | HomeSnapshotExportJobPhase::Failed => true,
                    HomeSnapshotExportJobPhase::Exporting => {
                        job.phase == HomeSnapshotExportJobPhase::Pending
                    }
                    HomeSnapshotExportJobPhase::Pending => false,
                }
        }) {
            return false;
        }
        expected.job = next.job.clone();
    } else if old.uploaded != next.uploaded {
        if old.uploaded.is_some() || next.uploaded.is_none() {
            return false;
        }
        expected.uploaded = next.uploaded.clone();
    } else {
        return false;
    }
    expected == *next
}

fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.background_checkpoint")
}

#[cfg(test)]
#[path = "background_checkpoint_tests.rs"]
mod tests;
