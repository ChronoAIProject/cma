use std::time::{Duration, UNIX_EPOCH};

use super::Launch;
use crate::trigger::{
    CheckpointId, LifecycleCheckpointState, TriggerValidationError, WorkspaceCheckpoint,
    WorkspaceCheckpointFailure, WorkspaceCheckpointIntent, WorkspaceCheckpointPurpose,
    WorkspaceCheckpointReservation, LIFECYCLE_CHECKPOINT_INTERVAL_SECONDS,
};
use crate::{SessionId, SubjectId};

impl Launch {
    /// Advance the Launch's session link from durable Layer 2 runtime evidence.
    ///
    /// Runtime generations can skip when a provider create fails after its
    /// generation was reserved, so the invariant is monotonic rather than
    /// `current + 1`. This method is intentionally not exposed through a
    /// public command: only `SessionWorkspaceSource` receives the generation
    /// selected by the session authority path.
    pub fn synchronize_runtime_generation(
        &mut self,
        session_id: &SessionId,
        owner: &SubjectId,
        runtime_generation: u64,
        now: &str,
    ) -> Result<bool, TriggerValidationError> {
        if &self.owner != owner || self.state.is_terminal() {
            return Err(TriggerValidationError::field(
                "session.runtime_generation",
                "does not belong to an active Launch owner",
            ));
        }
        if runtime_generation == 0 || crate::seconds_from_rfc3339(now).is_none() {
            return Err(TriggerValidationError::field(
                "session.runtime_generation",
                "requires non-zero durable runtime authority and a valid timestamp",
            ));
        }
        let Some(session) = self.session.as_mut() else {
            // Initial provisioning precedes completion of the Session effect.
            // That effect records this same authoritative generation when it
            // attaches the deterministic session to the Launch.
            return Ok(false);
        };
        if &session.session_id != session_id {
            return Err(TriggerValidationError::field(
                "session.session_id",
                "does not match the deterministic Trigger session",
            ));
        }
        if session.runtime_generation == runtime_generation {
            return Ok(false);
        }
        if session.runtime_generation > runtime_generation {
            return Err(TriggerValidationError::field(
                "session.runtime_generation",
                "cannot move backward from newer Launch authority",
            ));
        }
        if self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.checkpoint_intent.as_ref())
            .is_some()
        {
            return Err(TriggerValidationError::field(
                "workspace.checkpoint_intent",
                "must settle against the old runtime before authority advances",
            ));
        }
        session.runtime_generation = runtime_generation;
        self.updated_at = now.to_string();
        Ok(true)
    }

    /// Reserve one background or pre-teardown capture without changing the
    /// explicit contribution-review intent. Existing reservations are resumed
    /// only by the same runtime generation and purpose.
    pub fn reserve_lifecycle_checkpoint(
        &mut self,
        purpose: WorkspaceCheckpointPurpose,
        session_id: &SessionId,
        owner: &SubjectId,
        runtime_generation: u64,
        now: &str,
    ) -> Result<WorkspaceCheckpointReservation, TriggerValidationError> {
        if !purpose.is_lifecycle() {
            return Err(TriggerValidationError::field(
                "checkpoint_intent.purpose",
                "is not a lifecycle checkpoint",
            ));
        }
        if &self.owner != owner || self.state.is_terminal() {
            return Err(TriggerValidationError::field(
                "checkpoint_intent",
                "does not belong to an active Launch owner",
            ));
        }
        let session = self.session.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "checkpoint_intent.session_id",
                "has no deterministic Trigger session",
            )
        })?;
        if &session.session_id != session_id
            || runtime_generation == 0
            || runtime_generation != session.runtime_generation
        {
            return Err(TriggerValidationError::field(
                "checkpoint_intent.runtime_generation",
                "does not match the current Trigger session authority",
            ));
        }
        let workspace = self.workspace.as_mut().ok_or_else(|| {
            TriggerValidationError::field(
                "checkpoint_intent",
                "has no imported repository workspace",
            )
        })?;

        if workspace.lifecycle_checkpoint.state == LifecycleCheckpointState::RecoveryRequired {
            return Ok(WorkspaceCheckpointReservation::Busy);
        }

        if workspace.retired_checkpoint.is_some() {
            return Ok(WorkspaceCheckpointReservation::Busy);
        }

        if let Some(intent) = workspace.checkpoint_intent.as_ref() {
            if intent.purpose == purpose
                && &intent.session_id == session_id
                && intent.runtime_generation == runtime_generation
                && intent
                    .retry_at
                    .as_deref()
                    .is_none_or(|retry_at| retry_at <= now)
            {
                return Ok(WorkspaceCheckpointReservation::Capture(intent.clone()));
            }
            return Ok(WorkspaceCheckpointReservation::Busy);
        }

        if purpose == WorkspaceCheckpointPurpose::Periodic {
            let due = workspace
                .lifecycle_checkpoint
                .next_due_at
                .clone()
                .unwrap_or(deadline_after(
                    workspace
                        .lifecycle_checkpoint
                        .last_completed_at
                        .as_deref()
                        .or_else(|| {
                            workspace
                                .latest_checkpoint
                                .as_ref()
                                .map(|checkpoint| checkpoint.created_at.as_str())
                        })
                        .unwrap_or(session.created_at.as_str()),
                    LIFECYCLE_CHECKPOINT_INTERVAL_SECONDS,
                )?);
            if due.as_str() > now {
                return Ok(WorkspaceCheckpointReservation::NotDue);
            }
        }

        let high_water = workspace
            .latest_checkpoint
            .as_ref()
            .map_or(workspace.checkpoint_version, |checkpoint| {
                workspace.checkpoint_version.max(checkpoint.version)
            });
        let version = high_water.checked_add(1).ok_or_else(|| {
            TriggerValidationError::field(
                "checkpoint_intent.version",
                "cannot advance beyond the retained checkpoint high-water",
            )
        })?;
        let intent = WorkspaceCheckpointIntent {
            id: CheckpointId::new(),
            version,
            purpose,
            session_id: session_id.clone(),
            runtime_generation,
            workspace_epoch: workspace.workspace_epoch,
            requested_at: now.to_string(),
            retry_at: None,
        };
        workspace.checkpoint_intent = Some(intent.clone());
        workspace.lifecycle_checkpoint.state = LifecycleCheckpointState::Capturing;
        workspace.lifecycle_checkpoint.last_attempt_at = Some(now.to_string());
        workspace.lifecycle_checkpoint.last_failure = None;
        self.updated_at = now.to_string();
        Ok(WorkspaceCheckpointReservation::Capture(intent))
    }

    pub fn complete_lifecycle_checkpoint(
        &mut self,
        intent: &WorkspaceCheckpointIntent,
        checkpoint: Option<WorkspaceCheckpoint>,
        now: &str,
    ) -> Result<(), TriggerValidationError> {
        if !intent.purpose.is_lifecycle() {
            return Err(TriggerValidationError::field(
                "checkpoint_intent.purpose",
                "is not a lifecycle checkpoint",
            ));
        }
        let workspace = self.workspace.as_mut().ok_or_else(|| {
            TriggerValidationError::field("workspace", "is missing during checkpoint completion")
        })?;
        if workspace.checkpoint_intent.as_ref() != Some(intent) {
            return Err(TriggerValidationError::field(
                "checkpoint_intent",
                "does not match the durable lifecycle reservation",
            ));
        }
        let session = self.session.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "checkpoint_intent.session_id",
                "has no current Trigger session authority",
            )
        })?;
        if session.session_id != intent.session_id
            || session.runtime_generation != intent.runtime_generation
            || workspace.workspace_epoch != intent.workspace_epoch
        {
            return Err(TriggerValidationError::field(
                "checkpoint_intent",
                "is stale for the current Trigger session or workspace epoch",
            ));
        }
        if intent.version
            <= workspace
                .latest_checkpoint
                .as_ref()
                .map_or(workspace.checkpoint_version, |retained| {
                    workspace.checkpoint_version.max(retained.version)
                })
        {
            return Err(TriggerValidationError::field(
                "checkpoint_intent.version",
                "does not advance the capture high-water",
            ));
        }
        if let Some(checkpoint) = checkpoint {
            checkpoint.validate_complete(&self.id)?;
            if checkpoint.recovery_pair.is_none()
                || checkpoint.id != intent.id
                || checkpoint.version != intent.version
                || checkpoint.session_id != intent.session_id
                || checkpoint.runtime_generation != intent.runtime_generation
                || checkpoint.workspace_epoch != workspace.workspace_epoch
                || checkpoint.manifest.source_revision != self.source_sha
                || checkpoint.delete_after.as_deref() != Some(workspace.delete_after.as_str())
            {
                return Err(TriggerValidationError::field(
                    "checkpoint",
                    "does not match the exact lifecycle reservation",
                ));
            }
            install_recovery_checkpoint(workspace, checkpoint)?;
        }
        workspace.checkpoint_version = intent.version;
        workspace.checkpoint_intent = None;
        workspace.lifecycle_checkpoint.state = LifecycleCheckpointState::Idle;
        workspace.lifecycle_checkpoint.last_completed_at = Some(now.to_string());
        workspace.lifecycle_checkpoint.next_due_at =
            Some(deadline_after(now, LIFECYCLE_CHECKPOINT_INTERVAL_SECONDS)?);
        workspace.lifecycle_checkpoint.last_failure = None;
        self.updated_at = now.to_string();
        Ok(())
    }

    /// A capture failure before object upload leaves no ambiguous object. The
    /// previous checkpoint remains authoritative while maintenance retries; a
    /// missing writer-resume proof instead requires explicit recovery.
    pub fn fail_lifecycle_checkpoint(
        &mut self,
        intent: &WorkspaceCheckpointIntent,
        failure: WorkspaceCheckpointFailure,
        recovery_required: bool,
    ) -> Result<(), TriggerValidationError> {
        let workspace = self.workspace.as_mut().ok_or_else(|| {
            TriggerValidationError::field("workspace", "is missing during checkpoint failure")
        })?;
        if workspace.checkpoint_intent.as_ref() != Some(intent) || !intent.purpose.is_lifecycle() {
            return Err(TriggerValidationError::field(
                "checkpoint_intent",
                "does not match the failed lifecycle reservation",
            ));
        }
        let session = self.session.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "checkpoint_intent.session_id",
                "has no current Trigger session authority",
            )
        })?;
        if session.session_id != intent.session_id
            || session.runtime_generation != intent.runtime_generation
            || workspace.workspace_epoch != intent.workspace_epoch
        {
            return Err(TriggerValidationError::field(
                "checkpoint_intent",
                "is stale for the current Trigger session or workspace epoch",
            ));
        }
        if !recovery_required {
            workspace.checkpoint_intent = None;
        }
        workspace.lifecycle_checkpoint.state = if recovery_required {
            LifecycleCheckpointState::RecoveryRequired
        } else {
            LifecycleCheckpointState::RetryScheduled
        };
        workspace.lifecycle_checkpoint.next_due_at = failure.retry_at.clone();
        workspace.lifecycle_checkpoint.last_failure = Some(failure.clone());
        self.updated_at = failure.observed_at;
        Ok(())
    }

    /// Fence an exact committed recovery pair whose runtime writer state can
    /// no longer be proven. The pair comparison prevents a delayed failure
    /// from poisoning a newer checkpoint.
    pub fn mark_committed_checkpoint_recovery_required(
        &mut self,
        reference: &crate::TriggerRecoveryPairRef,
        failure: WorkspaceCheckpointFailure,
    ) -> Result<(), TriggerValidationError> {
        if crate::seconds_from_rfc3339(&failure.observed_at).is_none() || failure.retry_at.is_some()
        {
            return Err(TriggerValidationError::field(
                "workspace.lifecycle_checkpoint.last_failure",
                "recovery-required evidence needs a valid observation and no automatic retry",
            ));
        }
        let workspace = self.workspace.as_mut().ok_or_else(|| {
            TriggerValidationError::field("workspace", "is missing during recovery fencing")
        })?;
        let checkpoint = workspace.latest_checkpoint.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "workspace.latest_checkpoint",
                "is missing during recovery fencing",
            )
        })?;
        if checkpoint.recovery_pair.as_ref() != Some(reference)
            || workspace.checkpoint_intent.is_some()
        {
            return Err(TriggerValidationError::field(
                "workspace.latest_checkpoint.recovery_pair",
                "does not match the exact settled lifecycle checkpoint",
            ));
        }
        reference.validate_checkpoint(checkpoint)?;
        workspace.lifecycle_checkpoint.state = LifecycleCheckpointState::RecoveryRequired;
        workspace.lifecycle_checkpoint.next_due_at = None;
        workspace.lifecycle_checkpoint.last_failure = Some(failure.clone());
        self.updated_at = failure.observed_at;
        Ok(())
    }
}

pub(super) fn install_recovery_checkpoint(
    workspace: &mut super::WorkspaceRecord,
    checkpoint: WorkspaceCheckpoint,
) -> Result<(), TriggerValidationError> {
    if workspace.retired_checkpoint.is_some() {
        return Err(TriggerValidationError::field(
            "workspace.retired_checkpoint",
            "must be deleted before another recovery checkpoint can be installed",
        ));
    }
    let obsolete = workspace.previous_checkpoint.take();
    if let Some(obsolete) = obsolete {
        let pinned = workspace
            .sealed_contribution_checkpoint
            .as_ref()
            .is_some_and(|sealed| sealed.id == obsolete.id);
        if !pinned {
            workspace.retired_checkpoint = Some(obsolete);
        }
    }
    workspace.previous_checkpoint = workspace.latest_checkpoint.take();
    workspace.latest_checkpoint = Some(checkpoint);
    Ok(())
}

fn deadline_after(now: &str, seconds: u64) -> Result<String, TriggerValidationError> {
    let observed = crate::seconds_from_rfc3339(now).ok_or_else(|| {
        TriggerValidationError::field("checkpoint.timestamp", "is not a CMA timestamp")
    })?;
    let deadline = observed.checked_add(seconds).ok_or_else(|| {
        TriggerValidationError::field("checkpoint.timestamp", "exceeds the supported clock")
    })?;
    Ok(crate::rfc3339_at(
        UNIX_EPOCH + Duration::from_secs(deadline),
    ))
}
