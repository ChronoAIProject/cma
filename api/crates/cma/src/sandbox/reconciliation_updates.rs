//! Daily managed-runtime updates: observation is durable and admission is idle-fenced.
use super::SandboxReconciler;
use layer2_core::sandbox::store::{SandboxImageRotationRequest, SandboxStoreError};
use layer2_core::{rfc3339_at, SandboxId};
use std::time::SystemTime;

impl SandboxReconciler {
    pub(super) async fn reconcile_runtime_update(
        &self,
        id: &SandboxId,
        now: SystemTime,
        automatic_updates: bool,
    ) -> Result<bool, SandboxStoreError> {
        let Some(release) = self.released.as_ref().map(|released| released.snapshot()) else {
            return Ok(false);
        };
        let target = &release.agent;
        let stamp = rfc3339_at(now);
        for _ in 0..4 {
            let Some(mut sandbox) = self.store.get_sandbox(id).await? else {
                return Ok(false);
            };
            let revision = sandbox.revision;
            if !automatic_updates
                && sandbox
                    .runtime_update_request()
                    .is_none_or(|request| request.automatic || request.completed_at.is_some())
            {
                return Ok(false);
            }
            if automatic_updates
                && sandbox.managed_desktop_runtime_confirmed()
                && self
                    .store
                    .managed_runtime_target(&sandbox, target)
                    .is_some()
                && sandbox
                    .check_daily_runtime_update(target, &stamp)
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
            {
                match self
                    .store
                    .save_sandbox_against_release(
                        revision,
                        sandbox.clone(),
                        &release,
                        layer2_core::released_runtime::ReleasedLane::Agent,
                    )
                    .await
                {
                    Ok(()) => {}
                    Err(SandboxStoreError::Conflict) => continue,
                    Err(error) => return Err(error),
                }
            }
            let Some(request) = sandbox
                .runtime_update_request()
                .filter(|request| request.completed_at.is_none())
            else {
                return Ok(false);
            };
            if request.started_at.is_none() && request.failure.is_none() {
                // The store transaction rereads every Agent and fences message
                // admission before moving the Sandbox into rotation.
                self.store
                    .schedule_image_rotation(SandboxImageRotationRequest {
                        sandbox_id: id.clone(),
                        requested_at: stamp,
                        expected_runtime_version: request.target_version.clone(),
                    })
                    .await?;
            }
            return Ok(true);
        }
        // Contention means user activity won. Try again on the next maintenance pass.
        Ok(true)
    }
}
