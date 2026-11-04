//! Daily managed-runtime updates: observation is durable and admission is idle-fenced.
use super::SandboxReconciler;
use layer2_core::sandbox::store::{SandboxImageRotationRequest, SandboxStoreError};
use layer2_core::{rfc3339_at, SandboxId};
use std::time::SystemTime;

impl SandboxReconciler {
    /// Run only in the background sweep, after ordinary adoption. Existing
    /// failed older-image incidents require explicit owner/operator retargeting.
    pub(super) async fn recover_failed_unpublished_runtime(&self) -> Result<(), SandboxStoreError> {
        let Some(service) = &self.failed_runtime_recovery else {
            return Ok(());
        };
        let authority = self.authority().await?;
        let eligible = authority
            .values()
            .filter(|sandbox| {
                sandbox.unpublished_recovery_candidate()
                    && !sandbox.lifecycle.unpublished_recovery_attempted
                    && sandbox.lifecycle.unpublished_recovery.is_none()
                    && sandbox.admitted_runtime_update_target().or_else(|| {
                        sandbox
                            .runtime
                            .as_ref()
                            .map(|binding| binding.runtime_version.as_str())
                    }) == Some(service.latest_version())
            })
            .collect::<Vec<_>>();
        let Some(index) = recovery_schedule_index(&self.recovery_schedule_cursor, eligible.len())
        else {
            return Ok(());
        };
        let sandbox = eligible[index];
        // Scheduling rotates even if an earlier candidate cannot prove safety.
        // The cursor chooses only which candidate to inspect; admission rereads and
        // fences the complete durable state and never trusts this observation.
        match service.recover_failed_unpublished(&sandbox.id).await {
            Ok(_) => {
                tracing::info!(sandbox_id = %sandbox.id, "failed unpublished runtime recovery admitted")
            }
            Err(error) => {
                tracing::warn!(sandbox_id = %sandbox.id, error = %error, "failed runtime retained while recovery evidence is unavailable")
            }
        }
        Ok(())
    }

    pub(super) async fn reconcile_runtime_update(
        &self,
        id: &SandboxId,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let Some(target) = &self.runtime_version else {
            return Ok(false);
        };
        let stamp = rfc3339_at(now);
        for _ in 0..4 {
            let Some(mut sandbox) = self.store.get_sandbox(id).await? else {
                return Ok(false);
            };
            let revision = sandbox.revision;
            if sandbox
                .check_daily_runtime_update(target, &stamp)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
            {
                match self.store.save_sandbox(revision, sandbox.clone()).await {
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

fn recovery_schedule_index(cursor: &std::sync::atomic::AtomicUsize, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % count)
}

#[cfg(test)]
mod recovery_scheduling_tests {
    use super::*;
    #[test]
    fn refusal_and_skipped_clock_ticks_cannot_starve_later_candidates() {
        let cursor = std::sync::atomic::AtomicUsize::new(0);
        let indexes = (0..6)
            .map(|_| recovery_schedule_index(&cursor, 3))
            .collect::<Vec<_>>();
        assert_eq!(
            indexes,
            vec![Some(0), Some(1), Some(2), Some(0), Some(1), Some(2)]
        );
        assert_eq!(recovery_schedule_index(&cursor, 0), None);
    }
}
