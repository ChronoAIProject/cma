//! Lifecycle-fenced capture of one Trigger session's repository workspace.

use layer3_adapter::{WorkspaceCheckpointPackage, WorkspaceCheckpointRequest};

use crate::{CoreError, LaunchId, SessionId, SessionService, SubjectId};

impl SessionService {
    /// Capture through the exact runtime generation named by the Launch.
    ///
    /// The lifecycle guard spans authority validation, Layer 4 quiescence, and
    /// acknowledgement validation, so rotation cannot substitute a different
    /// sandbox anywhere inside the checkpoint boundary.
    pub async fn checkpoint_trigger_workspace(
        &self,
        session_id: &SessionId,
        launch_id: &LaunchId,
        owner: &SubjectId,
        expected_runtime_generation: u64,
        request: &WorkspaceCheckpointRequest,
    ) -> Result<(WorkspaceCheckpointPackage, u64), CoreError> {
        let _lifecycle = self.lock_lifecycle(session_id).await;
        let record = self.record(session_id).await?;
        let origin = record
            .trigger_origin
            .as_ref()
            .ok_or(CoreError::InvalidState)?;
        let runtime_generation = u64::from(record.session.rotations).saturating_add(1);
        if &record.session.owner != owner
            || &origin.launch_id != launch_id
            || origin.workspace_epoch != request.epoch
            || !origin
                .source_revision
                .eq_ignore_ascii_case(&request.source_revision)
            || !origin
                .manifest_digest
                .eq_ignore_ascii_case(&request.baseline_manifest_digest)
            || runtime_generation < expected_runtime_generation
        {
            return Err(CoreError::InvalidRequest(
                "the checkpoint request does not match the Trigger session authority".into(),
            ));
        }
        let (runtime, kernel) = {
            let entries = self.entries.read().await;
            let entry = entries.get(session_id).ok_or_else(|| {
                CoreError::RuntimeUnavailable(
                    "the Trigger session has no live runtime entry".into(),
                )
            })?;
            if entry.trigger_origin.as_ref() != Some(origin) {
                return Err(CoreError::InvalidRequest(
                    "the live runtime no longer matches the Trigger session origin".into(),
                ));
            }
            let runtime = entry.runtime.clone().ok_or_else(|| {
                CoreError::RuntimeUnavailable("the Trigger session has no current runtime".into())
            })?;
            (runtime, entry.session.kernel.clone())
        };
        let adapter = self
            .adapters
            .get(&kernel)
            .ok_or_else(|| CoreError::CapabilityUnsupported("workspace_checkpoint_v2".into()))?;
        let checkpoint = adapter
            .checkpoint_workspace(&runtime, request)
            .await
            .map_err(|error| match error {
                layer3_adapter::AdapterError::Unavailable(detail) => {
                    CoreError::RuntimeUnavailable(detail)
                }
                layer3_adapter::AdapterError::Rejected(detail)
                | layer3_adapter::AdapterError::AtCapacity(detail) => {
                    CoreError::RuntimeRefused(detail)
                }
                layer3_adapter::AdapterError::Unsupported(_) => {
                    CoreError::CapabilityUnsupported("workspace_checkpoint_v2".into())
                }
            })?;
        if checkpoint.epoch != request.epoch
            || checkpoint.version != request.version
            || !checkpoint
                .source_revision
                .eq_ignore_ascii_case(&request.source_revision)
        {
            return Err(CoreError::RuntimeRefused(
                "the runtime acknowledged a different workspace checkpoint".into(),
            ));
        }
        Ok((checkpoint, runtime_generation))
    }
}
