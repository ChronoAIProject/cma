//! Lifecycle-fenced capture of one Trigger session's repository workspace.

use std::sync::Arc;

use layer3_adapter::{
    KernelAdapter, RuntimeHandle, WorkspaceCheckpointDisposition, WorkspaceCheckpointPackage,
    WorkspaceCheckpointRequest, WorkspaceWriterState,
};
use sha2::{Digest as _, Sha256};

use crate::{
    CoreError, LaunchId, SessionId, SessionService, StoreError, SubjectId,
    WorkspaceCheckpointCommitOutcome, WorkspaceCheckpointFailure, WorkspaceCheckpointPurpose,
    WorkspaceCheckpointReservation,
};

const MAX_COMPLETE_GIT_WORKSPACE_FILES: usize = 750_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWriterHoldReceipt {
    pub runtime: RuntimeHandle,
    pub resume_token: String,
}

#[derive(Debug, Clone)]
pub enum TriggerLifecycleCheckpointOutcome {
    NotDue,
    Busy,
    Complete {
        storage: WorkspaceCheckpointCommitOutcome,
        writer_hold: Option<WorkspaceWriterHoldReceipt>,
        recovery_pair: Box<crate::TriggerRecoveryPair>,
    },
}

impl SessionService {
    /// Resolve one exact Trigger recovery point from durable Launch authority.
    /// Ordinary sessions deliberately return `None`; a Trigger session never
    /// falls back to a baseline-only workspace or a blank conversation here.
    pub(crate) async fn resolve_trigger_recovery_pair_for(
        &self,
        record: &crate::store::SessionRecord,
        expected: Option<&crate::TriggerRecoveryPairRef>,
    ) -> Result<Option<crate::TriggerRecoveryPair>, StoreError> {
        let Some(origin) = record.trigger_origin.as_ref() else {
            if expected.is_some() {
                return Err(StoreError::Malformed(
                    "a non-Trigger session cannot name a Trigger recovery pair".into(),
                ));
            }
            return Ok(None);
        };
        let source = self.workspaces.as_ref().ok_or_else(|| {
            StoreError::Unavailable(
                "Trigger workspace and conversation recovery storage is not configured".into(),
            )
        })?;
        source
            .resolve_recovery_pair(&record.session.owner, origin, expected)
            .await
    }

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
        let (checkpoint, runtime_generation, _, _) = self
            .checkpoint_trigger_workspace_locked(
                session_id,
                launch_id,
                owner,
                expected_runtime_generation,
                request,
            )
            .await?;
        Ok((checkpoint, runtime_generation))
    }

    /// Reserve, capture, durably install, and phase-classify one periodic or
    /// pre-teardown Trigger workspace checkpoint.
    pub async fn checkpoint_trigger_lifecycle(
        &self,
        session_id: &SessionId,
        purpose: WorkspaceCheckpointPurpose,
        now: &str,
    ) -> Result<TriggerLifecycleCheckpointOutcome, CoreError> {
        let lifecycle = self.lock_lifecycle(session_id).await;
        self.checkpoint_trigger_lifecycle_while_locked(&lifecycle, session_id, purpose, now)
            .await
    }

    pub(crate) async fn checkpoint_trigger_lifecycle_while_locked(
        &self,
        _lifecycle: &tokio::sync::OwnedMutexGuard<()>,
        session_id: &SessionId,
        purpose: WorkspaceCheckpointPurpose,
        now: &str,
    ) -> Result<TriggerLifecycleCheckpointOutcome, CoreError> {
        if !purpose.is_lifecycle() || crate::seconds_from_rfc3339(now).is_none() {
            return Err(CoreError::InvalidRequest(
                "the lifecycle checkpoint purpose or timestamp is invalid".into(),
            ));
        }
        let record = self.record(session_id).await?;
        let origin = record
            .trigger_origin
            .as_ref()
            .cloned()
            .ok_or_else(CoreError::invalid_state)?;
        let runtime_generation = u64::from(record.session.rotations).saturating_add(1);
        let owner = record.session.owner.clone();
        let source = self
            .workspaces
            .as_ref()
            .ok_or_else(|| CoreError::CapabilityUnsupported("workspace_checkpoint_v4".into()))?;
        let intent = match source
            .reserve_checkpoint(&owner, &origin, runtime_generation, purpose, now)
            .await
            .map_err(workspace_store_core_error)?
        {
            WorkspaceCheckpointReservation::NotDue => {
                return Ok(TriggerLifecycleCheckpointOutcome::NotDue)
            }
            WorkspaceCheckpointReservation::Busy => {
                return Ok(TriggerLifecycleCheckpointOutcome::Busy)
            }
            WorkspaceCheckpointReservation::Capture(intent) => intent,
        };
        // Both halves must describe one instant. Layer 4 therefore retains
        // its writer/startup fence until the opaque conversation is captured
        // and the complete pair is durably committed. Periodic work resumes
        // the hold below; pre-teardown work carries it into authority switch.
        let disposition = WorkspaceCheckpointDisposition::HoldForTeardown;
        let request = WorkspaceCheckpointRequest {
            epoch: origin.workspace_epoch,
            version: intent.version,
            source_revision: origin.source_revision.clone(),
            baseline_manifest_digest: origin.manifest_digest.clone(),
            include_git_metadata: false,
            disposition,
        };
        let captured = self
            .checkpoint_trigger_workspace_locked(
                session_id,
                &origin.launch_id,
                &owner,
                runtime_generation,
                &request,
            )
            .await;
        let (captured, observed_generation, adapter, runtime) = match captured {
            Ok(captured) => captured,
            Err(error) => {
                let recovery_required = matches!(error, CoreError::RuntimeUnavailable(_));
                let failure = WorkspaceCheckpointFailure {
                    code: if recovery_required {
                        "workspace_capture_outcome_ambiguous".into()
                    } else {
                        "workspace_capture_refused".into()
                    },
                    detail: if recovery_required {
                        "CMA could not prove whether the runtime retained a writer hold.".into()
                    } else {
                        "The runtime refused the checkpoint before CMA uploaded an object.".into()
                    },
                    observed_at: now.to_string(),
                    retry_at: (!recovery_required).then(|| lifecycle_retry_at(now)),
                };
                source
                    .fail_checkpoint(&owner, &origin, &intent, failure, recovery_required)
                    .await
                    .map_err(workspace_store_core_error)?;
                return Err(error);
            }
        };

        let Some(resume_token) = captured.writer_resume_token.clone() else {
            return Err(self
                .reject_writer_acknowledgement(
                    source,
                    &owner,
                    &origin,
                    &intent,
                    &captured,
                    &adapter,
                    &runtime,
                    now,
                    "the paired checkpoint omitted its writer hold token",
                )
                .await);
        };
        if captured.writer_state != WorkspaceWriterState::HeldForTeardown {
            return Err(self
                .reject_writer_acknowledgement(
                    source,
                    &owner,
                    &origin,
                    &intent,
                    &captured,
                    &adapter,
                    &runtime,
                    now,
                    "the paired checkpoint did not retain its writer fence",
                )
                .await);
        }
        let hold = WorkspaceWriterHoldReceipt {
            runtime: runtime.clone(),
            resume_token,
        };

        let conversation = match adapter.snapshot(&runtime).await {
            Ok(Some(conversation))
                if !conversation.is_empty()
                    && u64::try_from(conversation.len()).unwrap_or(u64::MAX)
                        <= crate::MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES =>
            {
                conversation
            }
            result => {
                let capture_error = match result {
                    Ok(None) | Ok(Some(_)) => CoreError::RuntimeRefused(
                        "the runtime returned no bounded recoverable conversation".into(),
                    ),
                    Err(layer3_adapter::AdapterError::Unavailable(detail)) => {
                        CoreError::RuntimeUnavailable(detail)
                    }
                    Err(layer3_adapter::AdapterError::Unsupported(detail)) => {
                        CoreError::CapabilityUnsupported(detail)
                    }
                    Err(layer3_adapter::AdapterError::Rejected(detail))
                    | Err(layer3_adapter::AdapterError::AtCapacity(detail)) => {
                        CoreError::RuntimeRefused(detail)
                    }
                };
                let resumed = adapter
                    .resume_workspace_writers(&runtime, &hold.resume_token)
                    .await
                    .is_ok();
                let failure = WorkspaceCheckpointFailure {
                    code: if resumed {
                        "conversation_capture_refused".into()
                    } else {
                        "workspace_writer_resume_unproven".into()
                    },
                    detail: if resumed {
                        "The runtime did not provide the conversation half of the checkpoint pair."
                            .into()
                    } else {
                        "Conversation capture failed and CMA could not prove that workspace writers reopened."
                            .into()
                    },
                    observed_at: now.to_string(),
                    retry_at: resumed.then(|| lifecycle_retry_at(now)),
                };
                source
                    .fail_checkpoint(&owner, &origin, &intent, failure, !resumed)
                    .await
                    .map_err(workspace_store_core_error)?;
                return Err(if resumed {
                    capture_error
                } else {
                    CoreError::RuntimeUnavailable(
                        "conversation capture failed and writer resumption is unproven".into(),
                    )
                });
            }
        };

        let committed = source
            .commit_checkpoint(
                &owner,
                &origin,
                &intent,
                observed_generation,
                &captured,
                &conversation,
                now,
            )
            .await;
        let committed = match committed {
            Ok(committed) => committed,
            Err(error) => {
                if let Err(resume_error) = adapter
                    .resume_workspace_writers(&runtime, &hold.resume_token)
                    .await
                {
                    let failure = WorkspaceCheckpointFailure {
                        code: "workspace_writer_resume_unproven".into(),
                        detail: "CMA could not prove that the old runtime reopened after checkpoint commit failed."
                            .into(),
                        observed_at: now.to_string(),
                        retry_at: None,
                    };
                    let _ = source
                        .fail_checkpoint(&owner, &origin, &intent, failure, true)
                        .await;
                    return Err(CoreError::RuntimeRefused(format!(
                        "checkpoint commit failed and the old runtime could not be reopened: {resume_error}"
                    )));
                }
                // Upload or Launch-CAS ambiguity deliberately retains the
                // exact intent. A subsequent run recaptures the same version
                // and writes the same immutable destination.
                return Err(workspace_store_core_error(error));
            }
        };
        let recovery_pair = crate::TriggerRecoveryPair {
            reference: committed.reference,
            workspace: layer3_adapter::WorkspacePackage {
                epoch: captured.epoch,
                source_revision: captured.source_revision.clone(),
                baseline_manifest_digest: origin.manifest_digest.clone(),
                manifest_digest: captured.manifest_digest.clone(),
                archive_sha256: captured.archive_sha256.clone(),
                archive: captured.archive.clone(),
            },
            conversation,
        };
        recovery_pair
            .validate_for(&origin)
            .map_err(|error| CoreError::RuntimeRefused(error.to_string()))?;
        let writer_hold = match purpose {
            WorkspaceCheckpointPurpose::Periodic => {
                if let Err(error) = adapter
                    .resume_workspace_writers(&runtime, &hold.resume_token)
                    .await
                {
                    let failure = WorkspaceCheckpointFailure {
                        code: "workspace_writer_resume_unproven".into(),
                        detail: "The recovery pair committed, but CMA could not prove that periodic capture reopened workspace writers."
                            .into(),
                        observed_at: now.to_string(),
                        retry_at: None,
                    };
                    source
                        .mark_checkpoint_recovery_required(
                            &owner,
                            &origin,
                            &recovery_pair.reference,
                            failure,
                        )
                        .await
                        .map_err(workspace_store_core_error)?;
                    return Err(CoreError::RuntimeUnavailable(format!(
                        "the paired checkpoint committed but writer resumption is unproven: {error}"
                    )));
                }
                None
            }
            WorkspaceCheckpointPurpose::PreTeardown => Some(hold),
            WorkspaceCheckpointPurpose::ContributionReview => unreachable!("validated above"),
        };
        Ok(TriggerLifecycleCheckpointOutcome::Complete {
            storage: committed.storage,
            writer_hold,
            recovery_pair: Box::new(recovery_pair),
        })
    }

    /// Reject a malformed writer acknowledgement without losing track of a
    /// hold that the runtime may already have installed.
    #[allow(clippy::too_many_arguments)] // Every value binds the rejected hold to one intent.
    async fn reject_writer_acknowledgement(
        &self,
        source: &Arc<dyn crate::store::SessionWorkspaceSource>,
        owner: &SubjectId,
        origin: &crate::TriggerSessionOrigin,
        intent: &crate::WorkspaceCheckpointIntent,
        captured: &WorkspaceCheckpointPackage,
        adapter: &Arc<dyn KernelAdapter>,
        runtime: &RuntimeHandle,
        now: &str,
        detail: &str,
    ) -> CoreError {
        let resume_proven = if let Some(token) = captured.writer_resume_token.as_deref() {
            adapter
                .resume_workspace_writers(runtime, token)
                .await
                .is_ok()
        } else {
            captured.writer_state == WorkspaceWriterState::Resumed
        };
        let failure = WorkspaceCheckpointFailure {
            code: if resume_proven {
                "workspace_writer_acknowledgement_invalid".into()
            } else {
                "workspace_writer_resume_unproven".into()
            },
            detail: if resume_proven {
                detail.to_string()
            } else {
                format!("{detail}; CMA could not prove that workspace writers reopened")
            },
            observed_at: now.to_string(),
            retry_at: resume_proven.then(|| lifecycle_retry_at(now)),
        };
        if let Err(error) = source
            .fail_checkpoint(owner, origin, intent, failure, !resume_proven)
            .await
        {
            return workspace_store_core_error(error);
        }
        if resume_proven {
            CoreError::RuntimeRefused(detail.into())
        } else {
            CoreError::RuntimeUnavailable(format!(
                "{detail}; workspace writer resumption is unproven"
            ))
        }
    }

    async fn checkpoint_trigger_workspace_locked(
        &self,
        session_id: &SessionId,
        launch_id: &LaunchId,
        owner: &SubjectId,
        expected_runtime_generation: u64,
        request: &WorkspaceCheckpointRequest,
    ) -> Result<
        (
            WorkspaceCheckpointPackage,
            u64,
            Arc<dyn KernelAdapter>,
            RuntimeHandle,
        ),
        CoreError,
    > {
        let record = self.record(session_id).await?;
        let origin = record
            .trigger_origin
            .as_ref()
            .ok_or_else(CoreError::invalid_state)?;
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
            || runtime_generation != expected_runtime_generation
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
            if entry.runtime_generation != runtime_generation {
                return Err(CoreError::InvalidRequest(
                    "the live runtime generation no longer matches the durable Trigger session"
                        .into(),
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
            return Err(Self::settle_rejected_checkpoint_writer_state(
                adapter,
                &runtime,
                &checkpoint,
                "the runtime acknowledged a different workspace checkpoint",
            )
            .await);
        }
        Ok((checkpoint, runtime_generation, adapter.clone(), runtime))
    }

    async fn settle_rejected_checkpoint_writer_state(
        adapter: &Arc<dyn KernelAdapter>,
        runtime: &RuntimeHandle,
        checkpoint: &WorkspaceCheckpointPackage,
        detail: &str,
    ) -> CoreError {
        if let Some(token) = checkpoint.writer_resume_token.as_deref() {
            return match adapter.resume_workspace_writers(runtime, token).await {
                Ok(()) => CoreError::RuntimeRefused(detail.into()),
                Err(error) => CoreError::RuntimeUnavailable(format!(
                    "{detail}; exact-token writer resumption could not be proven: {error}"
                )),
            };
        }
        if checkpoint.writer_state == WorkspaceWriterState::Resumed {
            CoreError::RuntimeRefused(detail.into())
        } else {
            CoreError::RuntimeUnavailable(format!(
                "{detail}; the runtime reported held writers without a resume token"
            ))
        }
    }
}

fn lifecycle_retry_at(now: &str) -> String {
    let seconds = crate::seconds_from_rfc3339(now).unwrap_or_default();
    crate::rfc3339_at(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds.saturating_add(30)),
    )
}

fn workspace_store_core_error(error: StoreError) -> CoreError {
    match error {
        StoreError::Malformed(detail) => CoreError::InvalidRequest(detail),
        StoreError::Unavailable(detail) => CoreError::RuntimeUnavailable(detail),
        other => CoreError::RuntimeUnavailable(other.to_string()),
    }
}

/// Validate one adapter package independently of its transport parser.
///
/// Chat uses the complete-Git digest domain while Trigger deliberately keeps
/// the original domain, so the capture scope is part of the sealed identity.
pub(crate) fn validate_workspace_checkpoint_package(
    request: &WorkspaceCheckpointRequest,
    checkpoint: &WorkspaceCheckpointPackage,
) -> Result<(), CoreError> {
    let max_files = if request.include_git_metadata {
        MAX_COMPLETE_GIT_WORKSPACE_FILES
    } else {
        crate::MAX_REPOSITORY_FILES
    };
    if request.epoch == 0
        || request.version == 0
        || checkpoint.epoch != request.epoch
        || checkpoint.version != request.version
        || !checkpoint
            .source_revision
            .eq_ignore_ascii_case(&request.source_revision)
        || checkpoint.includes_git_metadata != request.include_git_metadata
        || checkpoint.archive.is_empty()
        || checkpoint.quiesced_writers == 0
        || !is_hex(&checkpoint.source_revision, 40)
        || !is_hex(&checkpoint.manifest_digest, 64)
        || !is_hex(&checkpoint.archive_sha256, 64)
        || checkpoint_digest(&checkpoint.archive) != checkpoint.archive_sha256
        || checkpoint.entries.len() > max_files
    {
        return Err(CoreError::RuntimeRefused(
            "the runtime returned a malformed workspace checkpoint".into(),
        ));
    }

    let mut total_bytes = 0_u64;
    let mut previous_path: Option<&str> = None;
    for entry in &checkpoint.entries {
        if entry.path.is_empty()
            || previous_path.is_some_and(|previous| previous >= entry.path.as_str())
            || !is_hex(&entry.content_sha256, 64)
            || entry
                .provider_object_id
                .as_deref()
                .is_some_and(|object| !is_hex(object, 40))
        {
            return Err(CoreError::RuntimeRefused(
                "the runtime returned malformed workspace entry metadata".into(),
            ));
        }
        previous_path = Some(&entry.path);
        match entry.kind {
            layer3_adapter::WorkspaceEntryKind::Regular
                if matches!(entry.mode, 0o100644 | 0o100755) && entry.symlink_target.is_none() => {}
            layer3_adapter::WorkspaceEntryKind::Symlink
                if entry.mode == 0o120000
                    && entry.symlink_target.as_deref().is_some_and(|target| {
                        u64::try_from(target.len()).ok() == Some(entry.size)
                            && checkpoint_digest(target.as_bytes()) == entry.content_sha256
                    }) => {}
            _ => {
                return Err(CoreError::RuntimeRefused(
                    "the runtime returned an unsupported workspace entry".into(),
                ))
            }
        }
        total_bytes = total_bytes.checked_add(entry.size).ok_or_else(|| {
            CoreError::RuntimeRefused("the workspace checkpoint byte count overflowed".into())
        })?;
    }
    if total_bytes != checkpoint.total_bytes || total_bytes > crate::MAX_REPOSITORY_BYTES {
        return Err(CoreError::RuntimeRefused(
            "the workspace checkpoint size does not match its entries".into(),
        ));
    }
    if checkpoint_manifest_digest(checkpoint)? != checkpoint.manifest_digest {
        return Err(CoreError::RuntimeRefused(
            "the workspace checkpoint manifest digest is invalid".into(),
        ));
    }
    Ok(())
}

fn checkpoint_manifest_digest(
    checkpoint: &WorkspaceCheckpointPackage,
) -> Result<String, CoreError> {
    let mut digest = Sha256::new();
    digest.update(b"cma-workspace-manifest-v1\0");
    if checkpoint.includes_git_metadata {
        digest.update(b"complete-git-workspace-v1\0");
    }
    checkpoint_hash_text(&mut digest, &checkpoint.source_revision)?;
    digest.update(
        u64::try_from(checkpoint.entries.len())
            .map_err(|_| CoreError::RuntimeRefused("workspace manifest is too large".into()))?
            .to_be_bytes(),
    );
    for entry in &checkpoint.entries {
        checkpoint_hash_text(&mut digest, &entry.path)?;
        digest.update([match entry.kind {
            layer3_adapter::WorkspaceEntryKind::Regular => 1,
            layer3_adapter::WorkspaceEntryKind::Symlink => 2,
        }]);
        digest.update(entry.mode.to_be_bytes());
        digest.update(entry.size.to_be_bytes());
        checkpoint_hash_text(&mut digest, &entry.content_sha256)?;
        checkpoint_hash_optional_text(&mut digest, entry.provider_object_id.as_deref())?;
        checkpoint_hash_optional_text(&mut digest, entry.symlink_target.as_deref())?;
    }
    Ok(checkpoint_hex(&digest.finalize()))
}

fn checkpoint_hash_text(digest: &mut Sha256, value: &str) -> Result<(), CoreError> {
    digest.update(
        u64::try_from(value.len())
            .map_err(|_| CoreError::RuntimeRefused("workspace manifest value is too large".into()))?
            .to_be_bytes(),
    );
    digest.update(value.as_bytes());
    Ok(())
}

fn checkpoint_hash_optional_text(
    digest: &mut Sha256,
    value: Option<&str>,
) -> Result<(), CoreError> {
    match value {
        Some(value) => {
            digest.update([1]);
            checkpoint_hash_text(digest, value)
        }
        None => {
            digest.update([0]);
            Ok(())
        }
    }
}

fn checkpoint_digest(bytes: &[u8]) -> String {
    checkpoint_hex(&Sha256::digest(bytes))
}

fn checkpoint_hex(bytes: &[u8]) -> String {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(CHARS[usize::from(byte >> 4)]));
        out.push(char::from(CHARS[usize::from(byte & 0x0f)]));
    }
    out
}

fn is_hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
