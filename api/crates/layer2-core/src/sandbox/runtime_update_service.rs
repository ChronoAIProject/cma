use layer3_adapter::recovery_diagnostics::{Observation, Reason, Stage};
use std::sync::Arc;

use super::store::{SandboxStore, SandboxStoreError};
use super::{
    Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxRevivePhase,
    SandboxStatus, SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoveryVerifier,
};
use crate::{released_runtime::ReleasedLane, Caller, Capability, SandboxId, SubjectId};

#[derive(Debug, thiserror::Error)]
pub enum SandboxRuntimeUpdateError {
    #[error("forbidden")]
    Forbidden,
    #[error("sandbox not found")]
    NotFound,
    #[error("runtime update target changed")]
    TargetChanged,
    #[error("sandbox runtime update is not available")]
    NotAvailable,
    #[error("sandbox dependency unavailable")]
    Unavailable,
}

pub struct SandboxRuntimeUpdateService {
    store: Arc<dyn SandboxStore>,
    released: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
    recovery: Option<Arc<dyn SandboxUnpublishedRecoveryVerifier>>,
}

impl SandboxRuntimeUpdateService {
    /// A service whose latest managed runtime is this fixed agent-lane pin.
    pub fn new(store: Arc<dyn SandboxStore>, latest_version: String) -> Self {
        Self::with_released(
            store,
            Arc::new(layer3_adapter::PinnedReleasedRuntime::new(
                latest_version,
                None,
            )),
        )
    }

    /// Capture the source once per decision so one request never mixes two releases.
    pub fn with_released(
        store: Arc<dyn SandboxStore>,
        released: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
    ) -> Self {
        Self {
            store,
            released,
            recovery: None,
        }
    }

    pub fn with_unpublished_recovery(
        mut self,
        verifier: Arc<dyn SandboxUnpublishedRecoveryVerifier>,
    ) -> Self {
        self.recovery = Some(verifier);
        self
    }

    pub fn latest_version(&self) -> String {
        self.released.snapshot().agent
    }

    pub fn latest_version_for(&self, sandbox: &Sandbox) -> Option<String> {
        sandbox
            .managed_runtime_lane()
            .unwrap_or(ReleasedLane::Agent)
            .version_in(&self.released.snapshot())
            .map(str::to_owned)
    }

    pub fn target_is_qualified(&self, sandbox: &Sandbox, target: &str) -> bool {
        self.store.managed_runtime_target(sandbox, target).is_some()
    }

    pub fn can_update(&self, sandbox: &Sandbox) -> bool {
        self.can_update_to(
            sandbox,
            self.latest_version_for(sandbox)
                .as_deref()
                .unwrap_or_default(),
        )
    }

    /// `can_update` against a release the caller already captured.
    pub fn can_update_to(&self, sandbox: &Sandbox, latest: &str) -> bool {
        if !sandbox.has_managed_runtime_image() || scoped_explicit_retry(sandbox) {
            return false;
        }
        if sandbox.unpublished_recovery_candidate() {
            let pinned = sandbox
                .runtime_update_request()
                .filter(|request| request.completed_at.is_none())
                .map(|request| request.target_version.as_str());
            return self.recovery.is_some()
                && self
                    .explicit_recovery_target(sandbox, pinned, latest)
                    .is_some();
        }
        if !sandbox.supports_retained_runtime_update() {
            return false;
        }
        if let Some(request) = sandbox
            .runtime_update_request()
            .filter(|r| r.completed_at.is_none())
        {
            return request.retained_transition.is_none()
                && request.failure.is_some()
                && sandbox.work.lease.is_none()
                && sandbox.admitted_runtime_update_target()
                    == Some(request.target_version.as_str());
        }
        matches!(
            sandbox.status,
            SandboxStatus::Active | SandboxStatus::Paused
        ) && self.target_is_qualified(sandbox, latest)
            && sandbox.managed_desktop_runtime_confirmed()
            && sandbox
                .retained_runtime_binding()
                .is_some_and(|binding| binding.runtime_version != latest)
    }

    /// Whether a retry of the pinned target may recover onto the observed release.
    pub fn retry_uses_latest(&self, sandbox: &Sandbox, pinned: &str, latest: &str) -> bool {
        sandbox.has_managed_runtime_image()
            && !scoped_explicit_retry(sandbox)
            && self.recovery.is_some()
            && sandbox.unpublished_recovery_candidate()
            && self
                .explicit_recovery_target(sandbox, Some(pinned), latest)
                .is_some_and(|target| target != pinned)
    }

    fn explicit_recovery_target<'a>(
        &self,
        sandbox: &Sandbox,
        requested: Option<&'a str>,
        latest: &'a str,
    ) -> Option<&'a str> {
        // A fresh Agent reopen recovery must use the current qualified release,
        // including when the original target already matches that release.
        if agent_reopen(sandbox) {
            return self.store.managed_runtime_target(sandbox, latest);
        }
        requested.or_else(|| self.store.managed_runtime_target(sandbox, latest))
    }

    pub async fn request(
        &self,
        caller: &Caller,
        id: &SandboxId,
        target_version: &str,
    ) -> Result<Sandbox, SandboxRuntimeUpdateError> {
        if !caller.authorization.allows(Capability::UseOwnSandboxes) {
            return Err(SandboxRuntimeUpdateError::Forbidden);
        }
        self.queue(id, target_version, Some(&caller.subject)).await
    }

    /// Queue the same retained-home update for an operator without impersonating its owner.
    pub async fn request_as_operator(
        &self,
        caller: &Caller,
        id: &SandboxId,
        target_version: &str,
    ) -> Result<Sandbox, SandboxRuntimeUpdateError> {
        if !caller.authorization.allows(Capability::ControlAnySandbox) {
            return Err(SandboxRuntimeUpdateError::Forbidden);
        }
        self.queue(id, target_version, None).await
    }

    /// Automatic repair is available once per unresolved recovery incident after
    /// ordinary retries stall or park/fail. Managed recovery uses the current
    /// release. Custom recovery reuses the validated migration image authority.
    pub async fn recover_failed_unpublished(
        &self,
        id: &SandboxId,
    ) -> Result<Sandbox, SandboxRuntimeUpdateError> {
        let sandbox = self
            .store
            .get_sandbox(id)
            .await
            .map_err(store_error)?
            .ok_or(SandboxRuntimeUpdateError::NotFound)?;
        let release = self.released.snapshot();
        let latest = sandbox
            .managed_runtime_lane()
            .and_then(|lane| lane.version_in(&release))
            .unwrap_or_default();
        let target = sandbox
            .automatic_unpublished_recovery_target_for_release(latest)
            .ok_or(SandboxRuntimeUpdateError::NotAvailable)?;
        if sandbox.has_managed_runtime_image() && !self.target_is_qualified(&sandbox, target) {
            return Err(SandboxRuntimeUpdateError::NotAvailable);
        }
        let released_runtime = sandbox.has_managed_runtime_image().then_some(release);
        self.recover_unpublished(&sandbox, target, true, released_runtime)
            .await
            .map_err(store_error)
    }

    async fn recover_unpublished(
        &self,
        sandbox: &Sandbox,
        target: &str,
        automatic: bool,
        released_runtime: Option<layer3_adapter::ReleasedRuntime>,
    ) -> Result<Sandbox, SandboxStoreError> {
        let verifier = self
            .recovery
            .as_ref()
            .ok_or(SandboxStoreError::InvalidTransaction(
                "unpublished runtime recovery is unavailable",
            ))?;
        let mut snapshot_observation = Observation::new(Stage::Snapshot);
        let snapshot = observed(
            &mut snapshot_observation,
            self.store
                .unpublished_runtime_recovery_snapshot(&sandbox.id)
                .await,
        )?;
        if snapshot.sandbox != *sandbox {
            snapshot_observation.finish(Reason::StaleChildren);
            return Err(SandboxStoreError::Conflict);
        }
        observed(&mut snapshot_observation, snapshot.validate())?;
        drop(snapshot_observation);
        let proof = verifier.verify(&snapshot).await?;
        let mut admission = Observation::new(Stage::Admission);
        observed(
            &mut admission,
            self.store
                .admit_unpublished_runtime_recovery(SandboxUnpublishedRecoveryRequest {
                    snapshot,
                    proof,
                    target_runtime_version: target.into(),
                    released_runtime,
                    automatic,
                    requested_at: crate::now_rfc3339(),
                })
                .await,
        )
    }

    async fn queue(
        &self,
        id: &SandboxId,
        target_version: &str,
        owner: Option<&SubjectId>,
    ) -> Result<Sandbox, SandboxRuntimeUpdateError> {
        for _ in 0..8 {
            // One release per attempt. An attempt whose release is superseded
            // before its write admits nothing and retries against the new one.
            let released = self.released.snapshot();
            let mut sandbox = self
                .store
                .get_sandbox(id)
                .await
                .map_err(store_error)?
                .filter(|sandbox| owner.is_none_or(|owner| sandbox.owner == *owner))
                .ok_or(SandboxRuntimeUpdateError::NotFound)?;
            // Custom authority is admitted only by the operator migration path,
            // including recovery. The ordinary latest-image route cannot select
            // a target before the custom model's stronger admission runs.
            let lane = sandbox
                .managed_runtime_lane()
                .ok_or(SandboxRuntimeUpdateError::NotAvailable)?;
            if scoped_explicit_retry(&sandbox) {
                return Err(SandboxRuntimeUpdateError::NotAvailable);
            }
            let latest = lane.version_in(&released).unwrap_or_default();
            let pinned = sandbox.runtime_update_request().is_some_and(|request| {
                request.retained_transition.is_none()
                    && request.completed_at.is_none()
                    && request.target_version == target_version
            });
            if !pinned && latest.is_empty() {
                return Err(SandboxRuntimeUpdateError::NotAvailable);
            }
            if !pinned && target_version != latest {
                return Err(SandboxRuntimeUpdateError::TargetChanged);
            }
            if !pinned
                && (!self.target_is_qualified(&sandbox, latest)
                    || !sandbox.managed_desktop_runtime_confirmed())
            {
                return Err(SandboxRuntimeUpdateError::NotAvailable);
            }
            if sandbox.unpublished_recovery_candidate() {
                let target = self
                    .explicit_recovery_target(&sandbox, Some(target_version), latest)
                    .ok_or(SandboxRuntimeUpdateError::NotAvailable)?;
                if self.released.require_current(&released).is_err() {
                    superseded_release(&released).await;
                    continue;
                }
                let authoritative = (target == latest).then(|| released.clone());
                match self
                    .recover_unpublished(&sandbox, target, false, authoritative)
                    .await
                {
                    Ok(recovered) => return Ok(recovered),
                    Err(SandboxStoreError::Conflict) => {
                        self.released.refresh_after_stale().await;
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Err(error) => return Err(store_error(error)),
                }
            }
            let revision = sandbox.revision;
            if !sandbox
                .request_runtime_update(target_version.into(), false, crate::now_rfc3339())
                .map_err(|_| SandboxRuntimeUpdateError::NotAvailable)?
            {
                return Ok(sandbox);
            }
            if self.released.require_current(&released).is_err() {
                superseded_release(&released).await;
                continue;
            }
            let saved = if pinned {
                self.store.save_sandbox(revision, sandbox.clone()).await
            } else {
                self.store
                    .save_sandbox_against_release(revision, sandbox.clone(), &released, lane)
                    .await
            };
            match saved {
                Ok(()) => return Ok(sandbox),
                Err(SandboxStoreError::Conflict) => {
                    self.released.refresh_after_stale().await;
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(SandboxRuntimeUpdateError::Unavailable)
    }
}

/// A captured release went stale before its write. Name it, then yield so the
/// retry observes the new release instead of spinning on the old one.
async fn superseded_release(released: &layer3_adapter::ReleasedRuntime) {
    tracing::info!(
        revision = released.revision,
        "runtime update attempt retried after the released runtime changed"
    );
    tokio::task::yield_now().await;
}

impl Sandbox {
    /// Discovery and the fresh admission read apply the same image policy.
    pub fn automatic_unpublished_recovery_target_for_release(&self, latest: &str) -> Option<&str> {
        if self.lifecycle.unpublished_recovery_attempted
            || self.lifecycle.unpublished_recovery.is_some()
        {
            return None;
        }
        let target = self.automatic_unpublished_recovery_target()?;
        let eligible = if !self.has_managed_runtime_image() {
            self.validate_custom_recovery_target(target).is_ok()
        } else {
            target == latest
        };
        eligible.then_some(target)
    }
}

fn agent_reopen(sandbox: &Sandbox) -> bool {
    sandbox.managed_runtime_lane() == Some(ReleasedLane::Agent)
        && matches!(
            sandbox.lifecycle.operation,
            Some(SandboxLifecycleOperation::ImageRotation {
                phase: SandboxImageRotationPhase::ConversationReopen,
                ..
            }) | Some(SandboxLifecycleOperation::Revive {
                phase: SandboxRevivePhase::ConversationReopen,
                ..
            })
        )
}

fn scoped_explicit_retry(sandbox: &Sandbox) -> bool {
    sandbox.lifecycle.runtime_migration_admission.is_some()
        && (sandbox.unpublished_recovery_candidate()
            || sandbox
                .runtime_update_request()
                .is_some_and(|request| request.completed_at.is_none() && request.failure.is_some()))
}

fn store_error(error: SandboxStoreError) -> SandboxRuntimeUpdateError {
    match error {
        SandboxStoreError::NotFound => SandboxRuntimeUpdateError::NotFound,
        SandboxStoreError::InvalidTransaction(_) => SandboxRuntimeUpdateError::NotAvailable,
        _ => SandboxRuntimeUpdateError::Unavailable,
    }
}

fn observed<T>(
    observation: &mut Observation,
    result: Result<T, SandboxStoreError>,
) -> Result<T, SandboxStoreError> {
    observation.finish(match &result {
        Ok(_) => Reason::Success,
        Err(SandboxStoreError::Unavailable(_)) => Reason::Unavailable,
        Err(SandboxStoreError::Conflict) => Reason::StaleChildren,
        Err(SandboxStoreError::Corrupt(_) | SandboxStoreError::SequenceConflict) => {
            Reason::CorruptHistory
        }
        Err(_) => Reason::Ineligible,
    });
    result
}
