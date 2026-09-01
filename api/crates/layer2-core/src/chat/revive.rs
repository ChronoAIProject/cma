//! Durable shared-host revival and replacement-runtime restoration.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{
    AdapterError, KernelAdapter, RuntimeEnvironment, RuntimeHandle, RuntimeIdentity, RuntimeMode,
    RuntimeOptions, RuntimePermission, RuntimeSpec, TermSize, WorkspaceCheckpointPackage,
    WorkspaceEntryPackage, WorkspacePackage, WorkspaceSetupReceipt, WorkspaceWriterState,
};

use super::checkpoints::{live_session_ids, ChatCaptureRefusal};
use super::host::generation_for;
use crate::event::{Event, EventKind};
use crate::session::{RecoveryState, RuntimeLossCause, SessionStatus, StopReason};
use crate::store::{ChatHostStore, Page, PageRequest, StoreError};
use crate::{
    ChatCheckpointConversation, ChatHostId, ChatHostListQuery, ChatHostOperatorNote,
    ChatHostRecord, ChatHostStatus, ChatPromptAdmission, ChatPromptDeliveryState,
    ChatProvisioningPhase, ChatReviveCause, ChatRevivePhase, ChatRuntimeTransition,
    ChatThreadRestoreFailure, ChatUnavailableCause, CoreError, RuntimeAuthority, SessionBoundary,
    SessionId, SubjectId, MAX_CHAT_PROMPT_ADMISSIONS, MAX_CHAT_PROMPT_BYTES,
};

const ADMISSION_POLL_INTERVAL: Duration = Duration::from_millis(25);
const ADMISSION_RETENTION_SECONDS: u64 = 60 * 60;

struct VerifiedConversation {
    session_id: SessionId,
    snapshot: Vec<u8>,
}

struct VerifiedCheckpoint {
    workspace: WorkspacePackage,
    conversations: Vec<VerifiedConversation>,
}

struct ChatRevivalFailure<'a> {
    phase: ChatRevivePhase,
    detail: &'static str,
    retryable: bool,
    provisioning_phase: Option<ChatProvisioningPhase>,
    runtime: Option<(&'a RuntimeHandle, &'a Arc<dyn KernelAdapter>)>,
}

impl crate::SessionService {
    /// Recover an active chat host whose exact provider runtime is no longer
    /// serving. Every thread records the loss before a replacement can start,
    /// and the latest verified checkpoint timestamp becomes the honest
    /// boundary exposed on the restored session records.
    pub(crate) async fn recover_lost_chat_host(
        self: &Arc<Self>,
        host_id: &ChatHostId,
        lost_runtime: &str,
        cause: Option<RuntimeLossCause>,
    ) -> Result<bool, CoreError> {
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(chat_host_store_unavailable)?;
        let Some(observed) = hosts
            .get(host_id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Ok(false);
        };
        if observed.status != ChatHostStatus::Active
            || observed.runtime_id.as_deref() != Some(lost_runtime)
        {
            return Ok(false);
        }
        let owner_guard = self.chat_host_guards.lock(&observed.owner).await;
        let Some(current) = hosts
            .get(host_id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Ok(false);
        };
        if current.status != ChatHostStatus::Active
            || current.runtime_id.as_deref() != Some(lost_runtime)
        {
            return Ok(false);
        }
        let adapter = self.adapters.get(&current.kernel).ok_or_else(|| {
            CoreError::ChatUnavailable(ChatUnavailableCause::CapabilityUnsupported(
                "configured chat kernel".into(),
            ))
        })?;
        adapter
            .terminate_if_identity(&RuntimeIdentity {
                handle: RuntimeHandle(lost_runtime.to_string()),
                session: current.id.0.clone(),
                generation: Some(generation_for(&current)),
            })
            .await
            .map_err(adapter_error)?;
        let runtime_generation = generation_for(&current)
            .get()
            .checked_add(1)
            .ok_or_else(|| CoreError::InvalidRequest("chat runtime generation exhausted".into()))?;
        let claim = format!("chrl_{}", uuid::Uuid::new_v4().simple());
        let now = crate::clock::now_rfc3339();
        let recovery_boundary = current
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.captured_at.clone());
        self.mark_chat_threads_runtime_lost(&current, lost_runtime, cause)
            .await?;

        let mut reviving = current.clone();
        reviving.status = ChatHostStatus::Reviving;
        reviving.runtime_id = None;
        reviving.runtime_generation = runtime_generation;
        reviving.rotated_at.clone_from(&recovery_boundary);
        reviving.last_transition_claim = Some(claim);
        reviving.transition_started_at = Some(now.clone());
        reviving.revive_cause = Some(ChatReviveCause::RuntimeLost);
        reviving.revive_phase = Some(ChatRevivePhase::CheckpointVerification);
        reviving.revive_failed_threads.clear();
        reviving.last_failure = Some(crate::ChatHostFailure {
            phase: ChatProvisioningPhase::RuntimeLost,
            command_index: None,
            retryable: true,
            cleanup_pending: false,
            occurred_at: now.clone(),
        });
        reviving.last_operator_note =
            recovery_boundary
                .as_ref()
                .map(|captured_at| ChatHostOperatorNote {
                    code: "recovery_required".into(),
                    detail: format!(
                        "Recovering from the verified checkpoint captured at {captured_at}."
                    ),
                    observed_at: now.clone(),
                });
        reviving.updated_at = now;
        let Some(claimed) = hosts
            .update_if_revision(current.revision, &reviving)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Ok(false);
        };

        {
            let mut entries = self.entries.write().await;
            for session_id in live_session_ids(&claimed) {
                entries.remove(&session_id);
            }
        }
        drop(owner_guard);
        self.drive_chat_revival(host_id).await?;
        Ok(true)
    }

    /// Recover a pause claim whose runtime disappeared before the newly held
    /// checkpoint was durably published. Only the previously published
    /// checkpoint remains authoritative; when none exists, the ordinary revive
    /// verifier settles every thread in a typed recovery-required state without
    /// provisioning a replacement.
    pub(crate) async fn recover_interrupted_chat_pause(
        self: &Arc<Self>,
        host_id: &ChatHostId,
        lost_runtime: &str,
        cause: Option<RuntimeLossCause>,
    ) -> Result<bool, CoreError> {
        if lost_runtime.is_empty() {
            return Err(CoreError::RuntimeUnavailable(
                "an interrupted chat pause lost its durable runtime identity".into(),
            ));
        }
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(chat_host_store_unavailable)?;
        let Some(observed) = hosts
            .get(host_id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Ok(false);
        };
        if !matches!(
            observed.status,
            ChatHostStatus::Pausing | ChatHostStatus::Paused
        ) || observed.revive_cause.is_some()
            || observed.last_transition_claim.is_none()
            || observed.runtime_id.as_deref() != Some(lost_runtime)
            || (observed.status == ChatHostStatus::Paused
                && observed.checkpoint.as_ref().is_some_and(|checkpoint| {
                    checkpoint.checkpoint_version == observed.checkpoint_sequence
                }))
        {
            return Ok(false);
        }
        let owner_guard = self.chat_host_guards.lock(&observed.owner).await;
        let Some(current) = hosts
            .get(host_id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Ok(false);
        };
        if !matches!(
            current.status,
            ChatHostStatus::Pausing | ChatHostStatus::Paused
        ) || current.revive_cause.is_some()
            || current.last_transition_claim != observed.last_transition_claim
            || current.runtime_id.as_deref() != Some(lost_runtime)
            || (current.status == ChatHostStatus::Paused
                && current.checkpoint.as_ref().is_some_and(|checkpoint| {
                    checkpoint.checkpoint_version == current.checkpoint_sequence
                }))
        {
            return Ok(false);
        }
        let adapter = self.adapters.get(&current.kernel).ok_or_else(|| {
            CoreError::ChatUnavailable(ChatUnavailableCause::CapabilityUnsupported(
                "configured chat kernel".into(),
            ))
        })?;
        adapter
            .terminate_if_identity(&RuntimeIdentity {
                handle: RuntimeHandle(lost_runtime.to_string()),
                session: current.id.0.clone(),
                generation: Some(generation_for(&current)),
            })
            .await
            .map_err(adapter_error)?;

        let recovery_boundary = current
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.captured_at.clone());
        self.mark_chat_threads_runtime_lost(&current, lost_runtime, cause)
            .await?;

        let runtime_generation = generation_for(&current)
            .get()
            .checked_add(1)
            .ok_or_else(|| CoreError::InvalidRequest("chat runtime generation exhausted".into()))?;
        let now = crate::clock::now_rfc3339();
        let mut reviving = current.clone();
        reviving.status = ChatHostStatus::Reviving;
        reviving.runtime_id = None;
        reviving.runtime_generation = runtime_generation;
        reviving.rotated_at.clone_from(&recovery_boundary);
        reviving.last_transition_claim = Some(format!("chrl_{}", uuid::Uuid::new_v4().simple()));
        reviving.transition_started_at = Some(now.clone());
        reviving.revive_cause = Some(ChatReviveCause::RuntimeLost);
        reviving.revive_phase = Some(ChatRevivePhase::CheckpointVerification);
        reviving.revive_failed_threads.clear();
        reviving.last_failure = Some(crate::ChatHostFailure {
            phase: ChatProvisioningPhase::RuntimeLost,
            command_index: None,
            retryable: true,
            cleanup_pending: false,
            occurred_at: now.clone(),
        });
        reviving.last_operator_note =
            recovery_boundary
                .as_ref()
                .map(|captured_at| ChatHostOperatorNote {
                    code: "recovery_required".into(),
                    detail: format!(
                        "Recovering from the verified checkpoint captured at {captured_at}."
                    ),
                    observed_at: now.clone(),
                });
        reviving.updated_at = now;
        let Some(claimed) = hosts
            .update_if_revision(current.revision, &reviving)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Ok(false);
        };
        {
            let mut entries = self.entries.write().await;
            for session_id in live_session_ids(&claimed) {
                entries.remove(&session_id);
            }
        }
        drop(owner_guard);
        self.drive_chat_revival(host_id).await?;
        Ok(true)
    }

    async fn mark_chat_threads_runtime_lost(
        &self,
        host: &ChatHostRecord,
        lost_runtime: &str,
        cause: Option<RuntimeLossCause>,
    ) -> Result<(), CoreError> {
        for slot in host.threads.iter().filter(|slot| !slot.removed) {
            loop {
                let mut record = self
                    .store
                    .get(&slot.session_id)
                    .await
                    .map_err(session_store_error)?
                    .ok_or(CoreError::SessionNotFound)?;
                if record
                    .chat_origin
                    .as_ref()
                    .is_none_or(|origin| origin.host_id != host.id)
                    || record.runtime_id.as_deref() != Some(lost_runtime)
                {
                    return Err(CoreError::RuntimeUnavailable(
                        "a lost chat thread no longer matches its host authority".into(),
                    ));
                }
                let loss_already_recorded = self
                    .store
                    .events_since(&slot.session_id, 0)
                    .await
                    .map_err(session_store_error)?
                    .iter()
                    .any(|event| {
                        matches!(
                            &event.kind,
                            crate::StoredEventKind::Known(EventKind::RuntimeLost {
                                runtime_id,
                                ..
                            }) if runtime_id == lost_runtime
                        )
                    });
                if loss_already_recorded {
                    break;
                }
                let events = vec![Event {
                    session_id: slot.session_id.clone(),
                    seq: 0,
                    kind: EventKind::RuntimeLost {
                        runtime_id: lost_runtime.to_string(),
                        cause,
                    },
                    appended_at: String::new(),
                }];
                match self.persist_durably(&mut record, events).await {
                    Ok(()) => break,
                    Err(StoreError::RevisionConflict { .. }) => continue,
                    Err(error) => return Err(session_store_error(error)),
                }
            }
        }
        Ok(())
    }

    /// Rotate one quiet active host whose recorded image no longer matches the
    /// deployment. The outgoing runtime remains authoritative through a held
    /// checkpoint and is deleted by exact host identity before revival starts.
    pub async fn chat_rotation_candidates(
        &self,
        page: PageRequest,
    ) -> Result<Page<ChatHostRecord>, CoreError> {
        let Some(target_version) = self.runtime_version.as_deref() else {
            return Ok(Page {
                items: Vec::new(),
                next: None,
            });
        };
        let hosts = self
            .chat_hosts
            .as_ref()
            .ok_or_else(chat_host_store_unavailable)?;
        let page = hosts
            .list_candidates(&ChatHostListQuery::new(page, vec![ChatHostStatus::Active]))
            .await
            .map_err(|_| chat_host_store_unavailable())?;
        Ok(Page {
            items: page
                .items
                .into_iter()
                .filter(|host| host.runtime_version.as_deref() != Some(target_version))
                .collect(),
            next: page.next,
        })
    }

    /// Rotate one active host whose recorded image no longer matches the
    /// deployment. Busy threads make the attempt a no-op until a later pass.
    pub async fn rotate_chat_host_on_image_drift(
        self: &Arc<Self>,
        host_id: &ChatHostId,
    ) -> Result<bool, CoreError> {
        let Some(target_version) = self.runtime_version.as_ref() else {
            return Ok(false);
        };
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(chat_host_store_unavailable)?;
        let Some(observed) = hosts
            .get(host_id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Ok(false);
        };
        if observed.status != ChatHostStatus::Active
            || observed.runtime_version.as_deref() == Some(target_version.as_str())
        {
            return Ok(false);
        }
        let session_ids = live_session_ids(&observed);
        if session_ids.is_empty() {
            return Ok(false);
        }
        let version = next_chat_checkpoint_version(&observed).ok_or_else(|| {
            CoreError::RuntimeUnavailable("chat checkpoint version exhausted".into())
        })?;
        let claim = format!("chro_{}", uuid::Uuid::new_v4().simple());
        let now = crate::clock::now_rfc3339();
        let mut proposal = observed.clone();
        proposal.status = ChatHostStatus::Pausing;
        proposal.last_transition_claim = Some(claim.clone());
        proposal.transition_started_at = Some(now.clone());
        proposal.revive_cause = Some(ChatReviveCause::ImageDrift);
        proposal.revive_phase = None;
        proposal.checkpoint_sequence = version;
        proposal.last_failure = None;
        proposal.last_operator_note = None;
        proposal.updated_at = now.clone();
        let Some(claimed) = hosts
            .update_if_revision(observed.revision, &proposal)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Ok(false);
        };
        self.drive_claimed_chat_image_rotation(&hosts, claimed)
            .await
    }

    /// Resume the exact durable image-rotation claim after a control-plane
    /// replacement. Reusing its checkpoint version makes Layer 4 return the
    /// same held capture instead of creating another authority boundary.
    pub(crate) async fn resume_chat_image_rotation(
        self: &Arc<Self>,
        host_id: &ChatHostId,
    ) -> Result<bool, CoreError> {
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(chat_host_store_unavailable)?;
        let Some(claimed) = hosts
            .get(host_id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
            .filter(|host| {
                host.status == ChatHostStatus::Pausing
                    && host.revive_cause == Some(ChatReviveCause::ImageDrift)
                    && host.last_transition_claim.is_some()
                    && host.runtime_id.is_some()
            })
        else {
            return Ok(false);
        };
        self.drive_claimed_chat_image_rotation(&hosts, claimed)
            .await
    }

    /// Finish an image rotation whose exact old runtime is already absent but
    /// whose held checkpoint was durably published before the process stopped.
    pub(crate) async fn finish_torn_down_chat_image_rotation(
        self: &Arc<Self>,
        host_id: &ChatHostId,
    ) -> Result<bool, CoreError> {
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(chat_host_store_unavailable)?;
        let Some(observed) = hosts
            .get(host_id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Ok(false);
        };
        if observed.status != ChatHostStatus::Pausing
            || observed.revive_cause != Some(ChatReviveCause::ImageDrift)
            || observed.last_transition_claim.is_none()
            || observed
                .checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.checkpoint_version)
                != Some(observed.checkpoint_sequence)
        {
            return Ok(false);
        }
        let owner_guard = self.chat_host_guards.lock(&observed.owner).await;
        let Some(current) = hosts
            .get(host_id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
            .filter(|current| {
                current.status == ChatHostStatus::Pausing
                    && current.revive_cause == Some(ChatReviveCause::ImageDrift)
                    && current.last_transition_claim == observed.last_transition_claim
                    && current.runtime_id == observed.runtime_id
                    && current
                        .checkpoint
                        .as_ref()
                        .map(|checkpoint| checkpoint.checkpoint_version)
                        == Some(current.checkpoint_sequence)
            })
        else {
            return Ok(false);
        };
        let records = self.chat_thread_records(&current).await?;
        if records
            .iter()
            .any(|record| record.session.status != SessionStatus::Rotating)
        {
            return Err(CoreError::RuntimeUnavailable(
                "a torn-down chat rotation has an unsealed thread boundary".into(),
            ));
        }
        let checkpoint = current
            .checkpoint
            .as_ref()
            .expect("the exact checkpoint predicate proved a reference");
        let mut reviving = current.clone();
        reviving.status = ChatHostStatus::Reviving;
        reviving.runtime_id = None;
        reviving.runtime_generation = generation_for(&current)
            .get()
            .checked_add(1)
            .ok_or_else(|| CoreError::InvalidRequest("chat runtime generation exhausted".into()))?;
        reviving.rotations = current
            .rotations
            .checked_add(1)
            .ok_or_else(|| CoreError::InvalidRequest("chat rotation count exhausted".into()))?;
        reviving.rotated_at = Some(checkpoint.captured_at.clone());
        reviving.revive_phase = Some(ChatRevivePhase::CheckpointVerification);
        reviving.updated_at = crate::clock::now_rfc3339();
        let Some(reviving) = hosts
            .update_if_revision(current.revision, &reviving)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Ok(false);
        };
        {
            let mut entries = self.entries.write().await;
            for session_id in live_session_ids(&reviving) {
                entries.remove(&session_id);
            }
        }
        drop(owner_guard);
        self.drive_chat_revival(host_id).await?;
        Ok(true)
    }

    async fn drive_claimed_chat_image_rotation(
        self: &Arc<Self>,
        hosts: &Arc<dyn ChatHostStore>,
        claimed: ChatHostRecord,
    ) -> Result<bool, CoreError> {
        let claim = claimed
            .last_transition_claim
            .clone()
            .ok_or(CoreError::ChatUnavailable(
                ChatUnavailableCause::RevivePending,
            ))?;
        let expected_runtime = claimed.runtime_id.clone();
        let session_ids = live_session_ids(&claimed);
        let version = claimed.checkpoint_sequence;
        let captured_at =
            claimed
                .transition_started_at
                .clone()
                .ok_or(CoreError::ChatUnavailable(
                    ChatUnavailableCause::RevivePending,
                ))?;
        let mut input_fences = Vec::with_capacity(session_ids.len());
        for session_id in &session_ids {
            input_fences.push(self.input_gates.gate(session_id).write_owned().await);
        }
        let owner_guard = self.chat_host_guards.lock(&claimed.owner).await;
        let claimed = hosts
            .get(&claimed.id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
            .filter(|current| {
                current.status == ChatHostStatus::Pausing
                    && current.last_transition_claim.as_deref() == Some(claim.as_str())
                    && current.revive_cause == Some(ChatReviveCause::ImageDrift)
                    && current.runtime_id == expected_runtime
                    && live_session_ids(current) == session_ids
            })
            .ok_or(CoreError::ChatUnavailable(
                ChatUnavailableCause::RevivePending,
            ))?;

        let records = self.chat_thread_records(&claimed).await?;
        let rollback_safe = records
            .iter()
            .all(|record| record.session.status != SessionStatus::Rotating);
        if records.iter().any(|record| {
            matches!(
                record.session.status,
                SessionStatus::Running | SessionStatus::Provisioning
            )
        }) {
            if rollback_safe {
                self.rollback_chat_rotation_host(
                    hosts,
                    &claimed.id,
                    &claim,
                    "Image rotation is waiting for active turns to settle.",
                )
                .await?;
            }
            return Ok(false);
        }

        let mut newly_rotating = Vec::with_capacity(records.len());
        for original in &records {
            if original.session.status == SessionStatus::Rotating {
                continue;
            }
            if let Err(error) = self.mark_chat_thread_rotating(original.clone()).await {
                if rollback_safe {
                    let restored = self.restore_rotating_chat_threads(&newly_rotating).await;
                    if restored {
                        self.rollback_chat_rotation_host(
                            hosts,
                            &claimed.id,
                            &claim,
                            "The chat threads could not enter image rotation.",
                        )
                        .await?;
                    }
                }
                return Err(error);
            }
            newly_rotating.push(original.clone());
        }
        {
            let mut entries = self.entries.write().await;
            for session_id in &session_ids {
                entries.remove(session_id);
            }
        }

        let rotating_records = self.chat_thread_records(&claimed).await?;
        let capture = match self
            .capture_claimed_chat_checkpoint(&claimed, &rotating_records, version, &captured_at)
            .await
        {
            Ok(capture) => capture,
            Err(ChatCaptureRefusal {
                detail,
                writers_reopened,
            }) => {
                let restored = if writers_reopened && rollback_safe {
                    self.restore_rotating_chat_threads(&records).await
                } else {
                    false
                };
                if writers_reopened && restored {
                    self.rollback_chat_rotation_host(hosts, &claimed.id, &claim, detail)
                        .await?;
                }
                return Err(CoreError::RuntimeRefused(detail.into()));
            }
        };

        let mut published = claimed.clone();
        published.checkpoint = Some(capture.reference.clone());
        published.updated_at = crate::clock::now_rfc3339();
        let Some(published) = hosts
            .update_if_revision(claimed.revision, &published)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            let writers_reopened = capture.resume_writers().await.is_ok();
            let restored = if writers_reopened && rollback_safe {
                self.restore_rotating_chat_threads(&records).await
            } else {
                false
            };
            if writers_reopened && restored {
                self.rollback_chat_rotation_host(
                    hosts,
                    &claimed.id,
                    &claim,
                    "The image-rotation checkpoint could not be published.",
                )
                .await?;
            }
            return Err(CoreError::ChatUnavailable(
                ChatUnavailableCause::RevivePending,
            ));
        };

        let identity = RuntimeIdentity {
            handle: capture.runtime.clone(),
            session: published.id.0.clone(),
            generation: Some(generation_for(&published)),
        };
        if capture
            .adapter
            .terminate_if_identity(&identity)
            .await
            .is_err()
        {
            let writers_reopened = capture.resume_writers().await.is_ok();
            let restored = if writers_reopened && rollback_safe {
                self.restore_rotating_chat_threads(&records).await
            } else {
                false
            };
            if writers_reopened && restored {
                self.rollback_chat_rotation_host(
                    hosts,
                    &claimed.id,
                    &claim,
                    "The outgoing chat runtime could not be fenced-deleted.",
                )
                .await?;
            }
            return Err(CoreError::RuntimeUnavailable(
                "the outgoing chat runtime could not be fenced-deleted".into(),
            ));
        }
        self.prune_previous_chat_checkpoint(&capture.objects, &claimed.id, &capture.reference)
            .await;

        let runtime_generation = generation_for(&published)
            .get()
            .checked_add(1)
            .ok_or_else(|| CoreError::InvalidRequest("chat runtime generation exhausted".into()))?;
        let rotations = published
            .rotations
            .checked_add(1)
            .ok_or_else(|| CoreError::InvalidRequest("chat rotation count exhausted".into()))?;
        let mut reviving = published.clone();
        reviving.status = ChatHostStatus::Reviving;
        reviving.runtime_id = None;
        reviving.runtime_generation = runtime_generation;
        reviving.rotations = rotations;
        reviving.rotated_at = Some(capture.reference.captured_at.clone());
        reviving.revive_phase = Some(ChatRevivePhase::CheckpointVerification);
        reviving.updated_at = crate::clock::now_rfc3339();
        let Some(_) = hosts
            .update_if_revision(published.revision, &reviving)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Err(CoreError::ChatUnavailable(
                ChatUnavailableCause::RevivePending,
            ));
        };
        drop(owner_guard);
        drop(input_fences);
        self.drive_chat_revival(&claimed.id).await?;
        Ok(true)
    }

    async fn mark_chat_thread_rotating(
        &self,
        mut record: crate::SessionRecord,
    ) -> Result<(), CoreError> {
        if record.session.status.is_terminal() || record.chat_origin.is_none() {
            return Err(CoreError::invalid_state());
        }
        record.session.status = SessionStatus::Rotating;
        record.session.stop_reason = None;
        let id = record.session.id.clone();
        self.persist_durably(
            &mut record,
            vec![
                Event {
                    session_id: id.clone(),
                    seq: 0,
                    kind: EventKind::RuntimeHandoverStarted,
                    appended_at: String::new(),
                },
                Event {
                    session_id: id,
                    seq: 0,
                    kind: EventKind::StatusChanged {
                        status: SessionStatus::Rotating,
                        stop_reason: None,
                        detail: Some("Rotating the shared chat runtime image.".into()),
                        capability: None,
                    },
                    appended_at: String::new(),
                },
            ],
        )
        .await
        .map_err(session_store_error)
    }

    async fn restore_rotating_chat_threads(
        self: &Arc<Self>,
        originals: &[crate::SessionRecord],
    ) -> bool {
        let mut restored = true;
        for original in originals.iter().rev() {
            let outcome = async {
                let mut current = self.store.get(&original.session.id).await?.ok_or_else(|| {
                    StoreError::SessionDeleted {
                        id: original.session.id.clone(),
                    }
                })?;
                if current.session.status == original.session.status
                    && current.runtime_id == original.runtime_id
                {
                    return Ok::<_, StoreError>(current);
                }
                if current.session.status != SessionStatus::Rotating
                    || current.runtime_id != original.runtime_id
                {
                    return Err(StoreError::RevisionConflict {
                        expected: original.revision,
                        actual: current.revision,
                    });
                }
                current.session.status = original.session.status;
                current.session.recovery_state = original.session.recovery_state;
                current.session.stop_reason = original.session.stop_reason;
                current.session.runtime_version = original.session.runtime_version.clone();
                current.session.rotations = original.session.rotations;
                current.session.rotated_at = original.session.rotated_at.clone();
                current.session.chat = original.session.chat.clone();
                current.chat_origin = original.chat_origin.clone();
                let id = current.session.id.clone();
                self.persist_durably(
                    &mut current,
                    vec![Event {
                        session_id: id,
                        seq: 0,
                        kind: EventKind::StatusChanged {
                            status: original.session.status,
                            stop_reason: original.session.stop_reason,
                            detail: Some(
                                "Shared chat image rotation was rolled back before teardown."
                                    .into(),
                            ),
                            capability: None,
                        },
                        appended_at: String::new(),
                    }],
                )
                .await?;
                Ok(current)
            }
            .await;
            match outcome {
                Ok(record) => {
                    if !self.entries.read().await.contains_key(&record.session.id)
                        && self.readopt(&record).await.is_err()
                    {
                        restored = false;
                    }
                }
                Err(error) => {
                    restored = false;
                    tracing::error!(
                        session_id = %original.session.id,
                        %error,
                        "chat image-rotation rollback could not restore a thread"
                    );
                }
            }
        }
        restored
    }

    async fn rollback_chat_rotation_host(
        &self,
        hosts: &Arc<dyn ChatHostStore>,
        host_id: &ChatHostId,
        claim: &str,
        detail: &'static str,
    ) -> Result<(), CoreError> {
        loop {
            let Some(current) = hosts
                .get(host_id)
                .await
                .map_err(|_| chat_host_store_unavailable())?
            else {
                return Ok(());
            };
            if current.last_transition_claim.as_deref() != Some(claim) {
                return Ok(());
            }
            let now = crate::clock::now_rfc3339();
            let mut active = current.clone();
            active.status = ChatHostStatus::Active;
            active.last_transition_claim = None;
            active.transition_started_at = None;
            active.revive_cause = None;
            active.revive_phase = None;
            active.last_operator_note = Some(ChatHostOperatorNote {
                code: "revive_refused".into(),
                detail: detail.into(),
                observed_at: now.clone(),
            });
            active.updated_at = now;
            match hosts.update_if_revision(current.revision, &active).await {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => continue,
                Err(_) => return Err(chat_host_store_unavailable()),
            }
        }
    }

    /// A new chat window is owner activity just like a message on an existing
    /// window. Revive the complete paused host first so the new worktree is
    /// allocated only after every retained conversation is back on one current
    /// runtime.
    pub(crate) async fn revive_chat_host_for_new_thread(
        self: &Arc<Self>,
        owner: &SubjectId,
    ) -> Result<(), CoreError> {
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(chat_host_store_unavailable)?;
        let Some(observed) = hosts
            .get_for_owner(owner)
            .await
            .map_err(|_| chat_host_store_unavailable())?
        else {
            return Ok(());
        };
        match observed.status {
            ChatHostStatus::Active | ChatHostStatus::Provisioning => return Ok(()),
            ChatHostStatus::Pausing => {
                return Err(CoreError::ChatUnavailable(
                    ChatUnavailableCause::RevivePending,
                ));
            }
            // A retired aggregate is deliberately reincarnated by
            // `ensure_chat_host` under current deployment settings.
            ChatHostStatus::Retired => return Ok(()),
            ChatHostStatus::Paused if observed.last_transition_claim.is_some() => {
                return Err(CoreError::ChatUnavailable(
                    ChatUnavailableCause::RevivePending,
                ));
            }
            ChatHostStatus::Paused | ChatHostStatus::Reviving => {}
        }

        if observed.status == ChatHostStatus::Paused {
            let owner_guard = self.chat_host_guards.lock(owner).await;
            let Some(mut current) = hosts
                .get(&observed.id)
                .await
                .map_err(|_| chat_host_store_unavailable())?
            else {
                return Err(CoreError::ChatUnavailable(ChatUnavailableCause::Retired));
            };
            match current.status {
                ChatHostStatus::Active | ChatHostStatus::Reviving => {}
                ChatHostStatus::Paused if current.last_transition_claim.is_none() => {
                    let next_generation = generation_for(&current)
                        .get()
                        .checked_add(1)
                        .ok_or_else(|| {
                            CoreError::InvalidRequest("chat runtime generation exhausted".into())
                        })?;
                    account_paused_image_drift(&mut current, self.runtime_version.as_deref())?;
                    let now = crate::clock::now_rfc3339();
                    current.status = ChatHostStatus::Reviving;
                    current.runtime_id = None;
                    current.runtime_generation = next_generation;
                    current.last_transition_claim =
                        Some(format!("chrv_{}", uuid::Uuid::new_v4().simple()));
                    current.transition_started_at = Some(now.clone());
                    current.revive_cause = Some(ChatReviveCause::OwnerMessage);
                    current.revive_phase = Some(ChatRevivePhase::CheckpointVerification);
                    current.revive_failed_threads.clear();
                    current.last_failure = None;
                    current.last_operator_note = None;
                    current.updated_at = now;
                    hosts
                        .update_if_revision(current.revision, &current)
                        .await
                        .map_err(|_| chat_host_store_unavailable())?
                        .ok_or(CoreError::ChatUnavailable(
                            ChatUnavailableCause::RevivePending,
                        ))?;
                }
                ChatHostStatus::Paused | ChatHostStatus::Pausing => {
                    return Err(CoreError::ChatUnavailable(
                        ChatUnavailableCause::RevivePending,
                    ));
                }
                ChatHostStatus::Provisioning => {
                    return Err(CoreError::ChatUnavailable(
                        ChatUnavailableCause::Provisioning,
                    ));
                }
                ChatHostStatus::Retired => {
                    return Err(CoreError::ChatUnavailable(ChatUnavailableCause::Retired));
                }
            }
            drop(owner_guard);
        }

        self.drive_chat_revival(&observed.id).await?;
        let settled = hosts
            .get(&observed.id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
            .ok_or(CoreError::ChatUnavailable(ChatUnavailableCause::Retired))?;
        if settled.status == ChatHostStatus::Active {
            Ok(())
        } else if let Some(failure) = settled.last_failure.as_ref() {
            Err(CoreError::ChatUnavailable(
                ChatUnavailableCause::ProvisioningFailed {
                    phase: failure.phase,
                    command_index: failure.command_index,
                    retryable: failure.retryable,
                },
            ))
        } else {
            Err(CoreError::ChatUnavailable(
                ChatUnavailableCause::RevivePending,
            ))
        }
    }

    /// Admit input against an automatically paused thread before any provider
    /// work begins, then wait for that exact admission's finite outcome.
    pub(crate) async fn send_prompt_to_paused_chat(
        self: &Arc<Self>,
        owner: &SubjectId,
        session_id: &SessionId,
        payload: String,
    ) -> Result<u64, CoreError> {
        if payload.len() > MAX_CHAT_PROMPT_BYTES {
            return Err(CoreError::InvalidRequest(format!(
                "chat prompt exceeds the {MAX_CHAT_PROMPT_BYTES}-byte durable admission bound"
            )));
        }
        let record = self.record(session_id).await?;
        if !record.session.is_owned_by(owner) {
            return Err(CoreError::Forbidden);
        }
        let origin = record
            .chat_origin
            .as_ref()
            .ok_or_else(CoreError::invalid_state)?;
        let host_id = origin.host_id.clone();
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(chat_host_store_unavailable)?;

        let admission_id = {
            loop {
                let current_record = self
                    .store
                    .get(session_id)
                    .await
                    .map_err(session_store_error)?
                    .ok_or(CoreError::SessionNotFound)?;
                if !current_record.session.is_owned_by(owner) {
                    return Err(CoreError::Forbidden);
                }
                let current_origin = current_record
                    .chat_origin
                    .as_ref()
                    .filter(|current| current.host_id == host_id)
                    .ok_or_else(CoreError::invalid_state)?;
                let mut host = hosts
                    .get(&host_id)
                    .await
                    .map_err(|_| chat_host_store_unavailable())?
                    .ok_or(CoreError::ChatUnavailable(ChatUnavailableCause::Retired))?;
                if host.owner != *owner
                    || !host.threads.iter().any(|slot| {
                        slot.session_id == *session_id
                            && slot.worktree_name == current_origin.worktree_name
                            && slot.worktree_path == current_origin.worktree_path
                            && !slot.removed
                    })
                {
                    return Err(CoreError::Forbidden);
                }

                match host.status {
                    ChatHostStatus::Active => {
                        if let Some(failure) = host
                            .revive_failed_threads
                            .iter()
                            .find(|failure| failure.session_id == *session_id)
                        {
                            return Err(chat_restore_failed(failure.phase, failure.retryable));
                        }
                    }
                    ChatHostStatus::Paused if host.last_transition_claim.is_none() => {}
                    ChatHostStatus::Reviving
                        if host.revive_phase == Some(ChatRevivePhase::RecoveryRequired) =>
                    {
                        if let Some(failure) = host.last_failure.as_ref() {
                            return Err(CoreError::ChatUnavailable(
                                ChatUnavailableCause::ProvisioningFailed {
                                    phase: failure.phase,
                                    command_index: failure.command_index,
                                    retryable: failure.retryable,
                                },
                            ));
                        }
                        return Err(chat_restore_failed(
                            ChatRevivePhase::RecoveryRequired,
                            false,
                        ));
                    }
                    ChatHostStatus::Reviving => {}
                    ChatHostStatus::Paused => {
                        return Err(CoreError::ChatUnavailable(
                            ChatUnavailableCause::RevivePending,
                        ));
                    }
                    ChatHostStatus::Pausing => {
                        return Err(CoreError::ChatUnavailable(
                            ChatUnavailableCause::RevivePending,
                        ));
                    }
                    ChatHostStatus::Provisioning => {
                        return Err(CoreError::ChatUnavailable(
                            ChatUnavailableCause::Provisioning,
                        ));
                    }
                    ChatHostStatus::Retired => {
                        return Err(CoreError::ChatUnavailable(ChatUnavailableCause::Retired));
                    }
                }

                prune_prompt_admissions(&mut host);
                if host.prompt_admissions.len() >= MAX_CHAT_PROMPT_ADMISSIONS {
                    return Err(CoreError::InvalidState {
                        detail: Some(
                            "the chat host's durable prompt admission queue is full".into(),
                        ),
                    });
                }
                let sequence = host
                    .prompt_admission_sequence
                    .checked_add(1)
                    .ok_or_else(|| {
                        CoreError::InvalidRequest("chat prompt sequence exhausted".into())
                    })?;
                let now = crate::clock::now_rfc3339();
                let admission = ChatPromptAdmission::new(
                    session_id.clone(),
                    payload.clone(),
                    sequence,
                    now.clone(),
                );
                let admission_id = admission.id.clone();
                host.prompt_admission_sequence = sequence;
                host.prompt_admissions.push(admission);
                if host.status == ChatHostStatus::Paused {
                    let next_generation =
                        generation_for(&host).get().checked_add(1).ok_or_else(|| {
                            CoreError::InvalidRequest("chat runtime generation exhausted".into())
                        })?;
                    account_paused_image_drift(&mut host, self.runtime_version.as_deref())?;
                    host.status = ChatHostStatus::Reviving;
                    host.runtime_generation = next_generation;
                    host.runtime_id = None;
                    host.last_transition_claim =
                        Some(format!("chrv_{}", uuid::Uuid::new_v4().simple()));
                    host.transition_started_at = Some(now.clone());
                    host.revive_cause = Some(ChatReviveCause::OwnerMessage);
                    host.revive_phase = Some(ChatRevivePhase::CheckpointVerification);
                    host.revive_failed_threads.clear();
                    host.last_failure = None;
                    host.last_operator_note = None;
                }
                host.updated_at = now;
                let expected = host.revision;
                match hosts
                    .update_if_revision(expected, &host)
                    .await
                    .map_err(|_| chat_host_store_unavailable())?
                {
                    Some(_) => break admission_id,
                    None => continue,
                }
            }
        };

        self.spawn_chat_revival(host_id.clone());
        loop {
            let host = hosts
                .get(&host_id)
                .await
                .map_err(|_| chat_host_store_unavailable())?
                .ok_or(CoreError::ChatUnavailable(ChatUnavailableCause::Retired))?;
            let Some(admission) = host
                .prompt_admissions
                .iter()
                .find(|admission| admission.id == admission_id)
            else {
                return Err(CoreError::RuntimeUnavailable(
                    "the durable chat prompt outcome is no longer retained".into(),
                ));
            };
            match admission.state {
                ChatPromptDeliveryState::Delivered => {
                    return admission.turn_started.ok_or_else(|| {
                        CoreError::RuntimeUnavailable(
                            "a delivered chat prompt has no turn boundary".into(),
                        )
                    });
                }
                ChatPromptDeliveryState::RecoveryRequired => {
                    if let Some(phase) = admission.failure_provisioning_phase {
                        return Err(CoreError::ChatUnavailable(
                            ChatUnavailableCause::ProvisioningFailed {
                                phase,
                                command_index: None,
                                retryable: admission.failure_retryable,
                            },
                        ));
                    }
                    return Err(chat_restore_failed(
                        admission
                            .failure_phase
                            .unwrap_or(ChatRevivePhase::RecoveryRequired),
                        admission.failure_retryable,
                    ));
                }
                ChatPromptDeliveryState::Queued | ChatPromptDeliveryState::DispatchFenced => {
                    if host.status == ChatHostStatus::Reviving
                        || (host.status == ChatHostStatus::Active
                            && admission.state == ChatPromptDeliveryState::Queued)
                    {
                        self.spawn_chat_revival(host_id.clone());
                    }
                    tokio::time::sleep(ADMISSION_POLL_INTERVAL).await;
                }
            }
        }
    }

    pub(crate) fn spawn_chat_revival(self: &Arc<Self>, host_id: ChatHostId) {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            if !service
                .chat_hosts_starting
                .write()
                .await
                .insert(host_id.clone())
            {
                return;
            }
            if let Err(error) = service.drive_chat_revival(&host_id).await {
                tracing::warn!(chat_host = %host_id, %error, "chat revival remains pending");
            }
            service.chat_hosts_starting.write().await.remove(&host_id);
        });
    }

    /// Resume a durable revive claim. Every external step is either idempotent
    /// or preceded by a persisted phase/runtime identity that reconciliation can
    /// inspect after a process replacement.
    async fn drive_chat_revival(self: &Arc<Self>, host_id: &ChatHostId) -> Result<(), CoreError> {
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(chat_host_store_unavailable)?;
        let observed = hosts
            .get(host_id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
            .ok_or(CoreError::ChatUnavailable(ChatUnavailableCause::Retired))?;
        let _owner_guard = self.chat_host_guards.lock(&observed.owner).await;
        let mut host = hosts
            .get(host_id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
            .ok_or(CoreError::ChatUnavailable(ChatUnavailableCause::Retired))?;

        if host.status == ChatHostStatus::Active {
            return self.dispatch_chat_prompts(&hosts, host).await;
        }
        if host.status != ChatHostStatus::Reviving {
            return Ok(());
        }
        if host.last_transition_claim.is_none() {
            host.last_transition_claim = Some(format!("chrv_{}", uuid::Uuid::new_v4().simple()));
            host.transition_started_at = Some(crate::clock::now_rfc3339());
            host.revive_cause
                .get_or_insert(ChatReviveCause::OwnerMessage);
            host.revive_phase = Some(ChatRevivePhase::CheckpointVerification);
            host.runtime_generation =
                generation_for(&host).get().checked_add(1).ok_or_else(|| {
                    CoreError::InvalidRequest("chat runtime generation exhausted".into())
                })?;
            host = hosts
                .update_if_revision(host.revision, &host)
                .await
                .map_err(|_| chat_host_store_unavailable())?
                .ok_or(CoreError::ChatUnavailable(
                    ChatUnavailableCause::RevivePending,
                ))?;
        }
        let claim = host
            .last_transition_claim
            .clone()
            .expect("a reviving host has a durable claim");
        if host.revive_phase == Some(ChatRevivePhase::RecoveryRequired) {
            return self
                .finish_failed_chat_revival_cleanup(&hosts, &host, &claim)
                .await;
        }

        let checkpoint = match self.verify_chat_checkpoint(&host).await {
            Ok(checkpoint) => checkpoint,
            Err(error) => {
                self.fail_chat_revival(
                    &hosts,
                    &host,
                    &claim,
                    ChatRevivalFailure {
                        phase: ChatRevivePhase::CheckpointVerification,
                        detail: "The durable chat checkpoint failed verification.",
                        retryable: false,
                        provisioning_phase: None,
                        runtime: None,
                    },
                )
                .await?;
                return Err(error);
            }
        };
        if let Err(error) = self.mark_chat_threads_recovering(&host).await {
            self.fail_chat_revival(
                &hosts,
                &host,
                &claim,
                ChatRevivalFailure {
                    phase: ChatRevivePhase::ThreadRestore,
                    detail: "The durable chat threads could not enter recovery.",
                    retryable: true,
                    provisioning_phase: None,
                    runtime: None,
                },
            )
            .await?;
            return Err(error);
        }
        host = self
            .advance_chat_revive_phase(&hosts, &host, &claim, ChatRevivePhase::RuntimeProvisioning)
            .await?;

        let adapter = self.adapters.get(&host.kernel).cloned().ok_or_else(|| {
            CoreError::ChatUnavailable(ChatUnavailableCause::CapabilityUnsupported(
                "configured chat kernel".into(),
            ))
        })?;
        let handle = match host.runtime_id.as_ref() {
            Some(runtime_id) => RuntimeHandle(runtime_id.clone()),
            None => match self
                .provision_chat_replacement(&host, &checkpoint.workspace, &adapter)
                .await
            {
                Ok(handle) => {
                    host.runtime_id = Some(handle.0.clone());
                    host.revive_phase = Some(ChatRevivePhase::ConversationRestore);
                    host.updated_at = crate::clock::now_rfc3339();
                    host = match hosts.update_if_revision(host.revision, &host).await {
                        Ok(Some(host)) => host,
                        Ok(None) | Err(_) => {
                            let _ = self
                                .terminate_chat_replacement(&host, &handle, &adapter)
                                .await;
                            return Err(chat_host_store_unavailable());
                        }
                    };
                    handle
                }
                Err(error) => {
                    self.fail_chat_revival(
                        &hosts,
                        &host,
                        &claim,
                        ChatRevivalFailure {
                            phase: ChatRevivePhase::RuntimeProvisioning,
                            detail: "The replacement chat runtime could not be provisioned.",
                            retryable: error.is_runtime_loss(),
                            provisioning_phase: Some(ChatProvisioningPhase::Runtime),
                            runtime: None,
                        },
                    )
                    .await?;
                    return Err(adapter_error(error));
                }
            },
        };

        host = self
            .advance_chat_revive_phase(&hosts, &host, &claim, ChatRevivePhase::ConversationRestore)
            .await?;
        let mut failures = Vec::new();
        let mut restored = HashSet::new();
        for conversation in &checkpoint.conversations {
            let current = self
                .store
                .get(&conversation.session_id)
                .await
                .map_err(session_store_error)?;
            let already_open = current.as_ref().is_some_and(|record| {
                record.runtime_id.as_deref() == Some(handle.0.as_str())
                    && matches!(
                        record.session.status,
                        SessionStatus::Ready | SessionStatus::Running
                    )
            });
            if already_open {
                restored.insert(conversation.session_id.clone());
                continue;
            }
            match adapter
                .restore_conversation(
                    &handle,
                    conversation.session_id.as_str(),
                    &conversation.snapshot,
                )
                .await
            {
                Ok(()) => {
                    restored.insert(conversation.session_id.clone());
                }
                Err(error) if error.is_runtime_loss() => {
                    self.fail_chat_revival(
                        &hosts,
                        &host,
                        &claim,
                        ChatRevivalFailure {
                            phase: ChatRevivePhase::ConversationRestore,
                            detail:
                                "The replacement disappeared while restoring chat conversations.",
                            retryable: true,
                            provisioning_phase: None,
                            runtime: Some((&handle, &adapter)),
                        },
                    )
                    .await?;
                    return Err(adapter_error(error));
                }
                Err(error) => failures.push(ChatThreadRestoreFailure {
                    session_id: conversation.session_id.clone(),
                    phase: ChatRevivePhase::ConversationRestore,
                    retryable: error.is_runtime_loss(),
                    occurred_at: crate::clock::now_rfc3339(),
                }),
            }
        }

        host = self
            .advance_chat_revive_phase(&hosts, &host, &claim, ChatRevivePhase::WorkspaceSetup)
            .await?;
        let setup = adapter
            .run_workspace_setup(&handle, &host.pinned_settings.settings.setup.commands)
            .await;
        if !valid_setup(setup.as_ref().ok()) {
            self.fail_chat_revival(
                &hosts,
                &host,
                &claim,
                ChatRevivalFailure {
                    phase: ChatRevivePhase::WorkspaceSetup,
                    detail: "The replacement chat workspace setup failed.",
                    retryable: setup
                        .as_ref()
                        .err()
                        .is_some_and(AdapterError::is_runtime_loss),
                    provisioning_phase: Some(ChatProvisioningPhase::WorkspaceSetup),
                    runtime: Some((&handle, &adapter)),
                },
            )
            .await?;
            return Err(CoreError::RuntimeRefused(
                "the replacement chat workspace setup failed".into(),
            ));
        }

        host = self
            .advance_chat_revive_phase(&hosts, &host, &claim, ChatRevivePhase::ThreadRestore)
            .await?;
        for conversation in &checkpoint.conversations {
            if !restored.contains(&conversation.session_id) {
                continue;
            }
            match self
                .restore_chat_thread(&host, &handle, &conversation.session_id)
                .await
            {
                Ok(()) => {}
                Err(error @ CoreError::RuntimeUnavailable(_)) => {
                    self.fail_chat_revival(
                        &hosts,
                        &host,
                        &claim,
                        ChatRevivalFailure {
                            phase: ChatRevivePhase::ThreadRestore,
                            detail: "The replacement disappeared while reopening chat threads.",
                            retryable: true,
                            provisioning_phase: None,
                            runtime: Some((&handle, &adapter)),
                        },
                    )
                    .await?;
                    return Err(error);
                }
                Err(error) => {
                    tracing::warn!(
                        chat_host = %host.id,
                        session_id = %conversation.session_id,
                        %error,
                        "one chat thread could not reopen on the replacement runtime"
                    );
                    failures.push(ChatThreadRestoreFailure {
                        session_id: conversation.session_id.clone(),
                        phase: ChatRevivePhase::ThreadRestore,
                        retryable: false,
                        occurred_at: crate::clock::now_rfc3339(),
                    });
                }
            }
        }
        failures.sort_by(|left, right| left.session_id.as_str().cmp(right.session_id.as_str()));
        failures.dedup_by(|left, right| left.session_id == right.session_id);
        for failure in &failures {
            self.mark_chat_thread_restore_failed(&host, failure).await;
        }

        host = self
            .advance_chat_revive_phase(&hosts, &host, &claim, ChatRevivePhase::PromptDelivery)
            .await?;
        if let Err(error) = self.install_chat_host_credential(&host.id, None).await {
            self.fail_chat_revival(
                &hosts,
                &host,
                &claim,
                ChatRevivalFailure {
                    phase: ChatRevivePhase::PromptDelivery,
                    detail: "The replacement chat credential could not be installed.",
                    retryable: revive_error_is_retryable(&error),
                    provisioning_phase: Some(ChatProvisioningPhase::Credential),
                    runtime: Some((&handle, &adapter)),
                },
            )
            .await?;
            return Err(error);
        }

        let now = crate::clock::now_rfc3339();
        host.status = ChatHostStatus::Active;
        host.runtime_id = Some(handle.0.clone());
        host.runtime_version = self.runtime_version.clone();
        host.revived_at = Some(now.clone());
        host.last_transition_claim = None;
        host.transition_started_at = None;
        host.revive_cause = None;
        host.revive_phase = None;
        host.revive_failed_threads = failures;
        host.last_failure = None;
        host.last_operator_note = None;
        host.updated_at = now;
        host = hosts
            .update_if_revision(host.revision, &host)
            .await
            .map_err(|_| chat_host_store_unavailable())?
            .ok_or(CoreError::ChatUnavailable(
                ChatUnavailableCause::RevivePending,
            ))?;

        self.dispatch_chat_prompts(&hosts, host).await
    }

    async fn verify_chat_checkpoint(
        &self,
        host: &ChatHostRecord,
    ) -> Result<VerifiedCheckpoint, CoreError> {
        let reference = host
            .checkpoint
            .as_ref()
            .ok_or_else(|| chat_restore_failed(ChatRevivePhase::CheckpointVerification, false))?;
        reference
            .validate()
            .map_err(|_| chat_restore_failed(ChatRevivePhase::CheckpointVerification, false))?;
        let authority = host
            .workspace_authority
            .as_ref()
            .ok_or_else(|| chat_restore_failed(ChatRevivePhase::CheckpointVerification, false))?;
        let expected = host
            .threads
            .iter()
            .filter(|slot| !slot.removed)
            .map(|slot| slot.session_id.clone())
            .collect::<HashSet<_>>();
        let captured = reference
            .conversations
            .iter()
            .map(|conversation| conversation.session_id.clone())
            .collect::<HashSet<_>>();
        if expected != captured {
            return Err(chat_restore_failed(
                ChatRevivePhase::CheckpointVerification,
                false,
            ));
        }
        let objects = self
            .chat_checkpoints
            .as_ref()
            .ok_or_else(|| chat_restore_failed(ChatRevivePhase::CheckpointVerification, true))?;
        let archive = objects
            .read_workspace(
                &host.id,
                &reference.workspace_object_reference,
                &reference.workspace_archive_sha256,
            )
            .await
            .map_err(session_store_error)?
            .ok_or_else(|| chat_restore_failed(ChatRevivePhase::CheckpointVerification, false))?;
        let mut captured_bytes = Vec::with_capacity(reference.conversations.len());
        let mut conversations = Vec::with_capacity(reference.conversations.len());
        for conversation in &reference.conversations {
            let snapshot = objects
                .read_conversation(
                    &conversation.session_id,
                    &conversation.object_reference,
                    &conversation.sha256,
                )
                .await
                .map_err(session_store_error)?
                .ok_or_else(|| {
                    chat_restore_failed(ChatRevivePhase::CheckpointVerification, false)
                })?;
            captured_bytes.push(ChatCheckpointConversation {
                session_id: conversation.session_id.clone(),
                object_reference: conversation.object_reference.clone(),
                conversation_id: conversation.conversation_id.clone(),
                snapshot: snapshot.clone(),
            });
            conversations.push(VerifiedConversation {
                session_id: conversation.session_id.clone(),
                snapshot,
            });
        }
        let verification = WorkspaceCheckpointPackage {
            epoch: authority.workspace_epoch,
            version: reference.checkpoint_version,
            source_revision: authority.source_revision.clone(),
            manifest_digest: reference.workspace_manifest_digest.clone(),
            archive_sha256: reference.workspace_archive_sha256.clone(),
            entries: Vec::<WorkspaceEntryPackage>::new(),
            total_bytes: u64::try_from(archive.len()).unwrap_or(u64::MAX),
            quiesced_writers: 0,
            writer_state: WorkspaceWriterState::Resumed,
            writer_resume_token: None,
            includes_git_metadata: true,
            archive: archive.clone(),
        };
        reference
            .validate_bytes(&verification, &captured_bytes)
            .map_err(|_| chat_restore_failed(ChatRevivePhase::CheckpointVerification, false))?;
        Ok(VerifiedCheckpoint {
            workspace: WorkspacePackage {
                epoch: authority.workspace_epoch,
                source_revision: authority.source_revision.clone(),
                baseline_manifest_digest: authority.baseline_manifest_digest.clone(),
                manifest_digest: reference.workspace_manifest_digest.clone(),
                archive_sha256: reference.workspace_archive_sha256.clone(),
                archive,
            },
            conversations,
        })
    }

    async fn provision_chat_replacement(
        &self,
        host: &ChatHostRecord,
        workspace: &WorkspacePackage,
        adapter: &Arc<dyn KernelAdapter>,
    ) -> Result<RuntimeHandle, AdapterError> {
        let capabilities = adapter.capabilities();
        if !capabilities.conversation || !capabilities.snapshot {
            return Err(AdapterError::Unsupported(
                "shared chat restoration requires conversation snapshots".into(),
            ));
        }
        let baseline = self.environments.baseline();
        let service_names = baseline
            .services()
            .iter()
            .map(|service| service.0.clone())
            .collect::<Vec<_>>();
        let wanted_skills = baseline
            .skills()
            .iter()
            .map(|skill| skill.0.clone())
            .collect::<Vec<_>>();
        let skill_packs = self
            .resolve_skill_packs_for_creation(None, &wanted_skills)
            .await
            .map_err(|_| AdapterError::Unavailable("chat skill resolution failed".into()))?;
        let services = baseline.services().to_vec();
        match self
            .credential_for(
                &host.id.credential_session_id(),
                &host.owner,
                &services,
                None,
            )
            .await
            .map_err(|_| AdapterError::Unavailable("chat credential resolution failed".into()))?
        {
            Some(credential) => drop(credential),
            None => {
                return Err(AdapterError::Rejected(
                    "the chat owner has no delegated credential".into(),
                ));
            }
        }
        let settings = &host.pinned_settings.settings;
        let spec = RuntimeSpec {
            session_id: host.id.0.clone(),
            chat_host: Some(host.id.0.clone()),
            generation: Some(generation_for(host)),
            reservation_token: None,
            size: TermSize {
                cols: 120,
                rows: 40,
            },
            capacity: crate::provision_support::runtime_capacity(settings.host.size),
            timeout_seconds: None,
            subject_credential: None,
            options: RuntimeOptions {
                mode: RuntimeMode::Conversation,
                model: settings.agent.model.clone(),
                reasoning_effort: Some(settings.agent.reasoning_effort.as_str().to_string()),
                permission: RuntimePermission::Unrestricted,
                own_credential: false,
                restricted_service_gateway: false,
                working_directory: None,
                conversation_slot: None,
                resume_conversation: None,
            },
            restore_from: None,
            workspace: Some(workspace.clone()),
            created_by: self.identity.clone(),
            skills: skill_packs
                .iter()
                .map(|pack| layer3_adapter::SkillPackage {
                    name: pack.name.clone(),
                    files: pack.files.clone(),
                })
                .collect(),
            platform_briefing: host.platform_briefing.clone(),
            environment: RuntimeEnvironment {
                variables: Default::default(),
                packages: baseline
                    .packages()
                    .iter()
                    .map(crate::SoftwarePackage::requirement)
                    .collect(),
                skills: wanted_skills,
                agent_notes: settings.agent.instructions.clone(),
            },
            reaches: service_names,
            runtime_policy_required: false,
            sponsored_model_required: false,
            workspace_source_required: true,
            conversation_slots_required: Some(settings.host.max_threads),
        };
        adapter.provision(&spec).await
    }

    async fn mark_chat_threads_recovering(&self, host: &ChatHostRecord) -> Result<(), CoreError> {
        for slot in host.threads.iter().filter(|slot| !slot.removed) {
            loop {
                let mut record = self
                    .store
                    .get(&slot.session_id)
                    .await
                    .map_err(session_store_error)?
                    .ok_or(CoreError::SessionNotFound)?;
                if record.session.owner != host.owner
                    || record.chat_origin.as_ref().is_none_or(|origin| {
                        origin.host_id != host.id
                            || origin.worktree_name != slot.worktree_name
                            || origin.worktree_path != slot.worktree_path
                    })
                {
                    return Err(CoreError::RuntimeUnavailable(
                        "a chat thread no longer matches its host restore authority".into(),
                    ));
                }
                if record.session.status == SessionStatus::Provisioning
                    && record.session.recovery_state == RecoveryState::Recovering
                {
                    break;
                }
                record.session.status = SessionStatus::Provisioning;
                record.session.recovery_state = RecoveryState::Recovering;
                record.session.stop_reason = None;
                let event = Event {
                    session_id: slot.session_id.clone(),
                    seq: 0,
                    kind: EventKind::StatusChanged {
                        status: SessionStatus::Provisioning,
                        stop_reason: None,
                        detail: Some("Restoring the shared chat runtime.".into()),
                        capability: None,
                    },
                    appended_at: String::new(),
                };
                match self.persist_durably(&mut record, vec![event]).await {
                    Ok(()) => break,
                    Err(StoreError::RevisionConflict { .. }) => continue,
                    Err(error) => return Err(session_store_error(error)),
                }
            }
        }
        Ok(())
    }

    async fn restore_chat_thread(
        self: &Arc<Self>,
        host: &ChatHostRecord,
        handle: &RuntimeHandle,
        session_id: &SessionId,
    ) -> Result<(), CoreError> {
        loop {
            let mut record = self
                .store
                .get(session_id)
                .await
                .map_err(session_store_error)?
                .ok_or(CoreError::SessionNotFound)?;
            if record.runtime_id.as_deref() == Some(handle.0.as_str())
                && matches!(
                    record.session.status,
                    SessionStatus::Ready | SessionStatus::Running
                )
            {
                if !self.entries.read().await.contains_key(session_id) {
                    self.readopt(&record).await?;
                }
                return Ok(());
            }
            let authority = match self.store.runtime_authority(session_id).await {
                Ok(Some(authority)) => authority,
                Ok(None) => {
                    return Err(CoreError::RuntimeUnavailable(
                        "a chat thread has no durable runtime authority".into(),
                    ))
                }
                Err(StoreError::CoordinationDisabled) => RuntimeAuthority {
                    generation: 0,
                    runtime_id: record.runtime_id.clone(),
                },
                Err(error) => return Err(session_store_error(error)),
            };
            if authority.runtime_id != record.runtime_id {
                return Err(CoreError::RuntimeUnavailable(
                    "a chat thread's runtime record and authority disagree".into(),
                ));
            }
            let replacement_generation = generation_for(host).get();
            if record.runtime_id.as_deref() == Some(handle.0.as_str()) {
                if authority.generation != 0 && authority.generation != replacement_generation {
                    return Err(CoreError::RuntimeUnavailable(
                        "a chat thread names the replacement handle at another generation".into(),
                    ));
                }
                self.entries.write().await.remove(session_id);
                self.readopt(&record).await?;
                return Ok(());
            }
            let previous = SessionBoundary::for_record(&record, authority.generation);
            let origin = record
                .chat_origin
                .as_mut()
                .ok_or_else(CoreError::invalid_state)?;
            if origin.host_id != host.id {
                return Err(CoreError::RuntimeUnavailable(
                    "a restored chat thread changed hosts".into(),
                ));
            }
            origin.paused_notice = None;
            record.session.chat = Some(origin.public_projection());
            record.runtime_id = Some(handle.0.clone());
            record.session.status = SessionStatus::Provisioning;
            record.session.recovery_state = RecoveryState::Recovering;
            record.session.stop_reason = None;
            record.session.runtime_version = self.runtime_version.clone();
            record.session.rotations = host.rotations;
            record.session.rotated_at.clone_from(&host.rotated_at);
            let runtime_started = Event {
                session_id: session_id.clone(),
                seq: 0,
                kind: EventKind::RuntimeIncarnationStarted,
                appended_at: String::new(),
            };
            let transition = ChatRuntimeTransition {
                host_id: host.id.clone(),
                previous,
                next: RuntimeAuthority {
                    generation: replacement_generation,
                    runtime_id: Some(handle.0.clone()),
                },
            };
            match self
                .persist_chat_runtime_transition_durably(
                    &mut record,
                    vec![runtime_started],
                    transition,
                )
                .await
            {
                Ok(()) => {
                    self.entries.write().await.remove(session_id);
                    self.readopt(&record).await?;
                    return Ok(());
                }
                Err(StoreError::RevisionConflict { .. }) => continue,
                Err(StoreError::SessionAuthorityChanged { .. }) => continue,
                Err(error) => return Err(session_store_error(error)),
            }
        }
    }

    async fn mark_chat_thread_restore_failed(
        &self,
        host: &ChatHostRecord,
        failure: &ChatThreadRestoreFailure,
    ) {
        for _ in 0..4 {
            let Ok(Some(mut record)) = self.store.get(&failure.session_id).await else {
                return;
            };
            if record
                .chat_origin
                .as_ref()
                .is_none_or(|origin| origin.host_id != host.id)
            {
                return;
            }
            record.session.status = SessionStatus::Idle;
            record.session.recovery_state = RecoveryState::Degraded;
            record.session.stop_reason = Some(StopReason::Error);
            let event = Event {
                session_id: failure.session_id.clone(),
                seq: 0,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Idle,
                    stop_reason: Some(StopReason::Error),
                    detail: Some(format!(
                        "Chat restoration failed during {:?}.",
                        failure.phase
                    )),
                    capability: None,
                },
                appended_at: String::new(),
            };
            match self.persist_durably(&mut record, vec![event]).await {
                Ok(()) => return,
                Err(StoreError::RevisionConflict { .. }) => continue,
                Err(_) => return,
            }
        }
    }

    async fn dispatch_chat_prompts(
        self: &Arc<Self>,
        hosts: &Arc<dyn ChatHostStore>,
        mut host: ChatHostRecord,
    ) -> Result<(), CoreError> {
        loop {
            if host
                .prompt_admissions
                .iter()
                .any(|admission| admission.state == ChatPromptDeliveryState::DispatchFenced)
            {
                return Ok(());
            }
            let Some(index) = host
                .prompt_admissions
                .iter()
                .position(|admission| admission.state == ChatPromptDeliveryState::Queued)
            else {
                return Ok(());
            };
            let admission = host.prompt_admissions[index].clone();
            if let Some(failure) = host
                .revive_failed_threads
                .iter()
                .find(|failure| failure.session_id == admission.session_id)
            {
                host.prompt_admissions[index].state = ChatPromptDeliveryState::RecoveryRequired;
                host.prompt_admissions[index].failure_phase = Some(failure.phase);
                host.prompt_admissions[index].failure_retryable = failure.retryable;
                host.updated_at = crate::clock::now_rfc3339();
                host = hosts
                    .update_if_revision(host.revision, &host)
                    .await
                    .map_err(|_| chat_host_store_unavailable())?
                    .ok_or(CoreError::ChatUnavailable(
                        ChatUnavailableCause::RevivePending,
                    ))?;
                continue;
            }

            let fenced_at = crate::clock::now_rfc3339();
            host.prompt_admissions[index].state = ChatPromptDeliveryState::DispatchFenced;
            host.prompt_admissions[index].dispatch_fenced_at = Some(fenced_at.clone());
            host.updated_at = fenced_at;
            host = hosts
                .update_if_revision(host.revision, &host)
                .await
                .map_err(|_| chat_host_store_unavailable())?
                .ok_or(CoreError::ChatUnavailable(
                    ChatUnavailableCause::RevivePending,
                ))?;
            let outcome = self
                .send_live_prompt(
                    &host.owner,
                    &admission.session_id,
                    admission.payload.clone(),
                )
                .await;
            loop {
                let mut current = hosts
                    .get(&host.id)
                    .await
                    .map_err(|_| chat_host_store_unavailable())?
                    .ok_or(CoreError::ChatUnavailable(ChatUnavailableCause::Retired))?;
                let Some(current_admission) = current
                    .prompt_admissions
                    .iter_mut()
                    .find(|candidate| candidate.id == admission.id)
                else {
                    return Err(CoreError::RuntimeUnavailable(
                        "the fenced chat prompt admission disappeared".into(),
                    ));
                };
                if current_admission.state != ChatPromptDeliveryState::DispatchFenced {
                    host = current;
                    break;
                }
                match outcome.as_ref() {
                    Ok(turn_started) => {
                        current_admission.state = ChatPromptDeliveryState::Delivered;
                        current_admission.turn_started = Some(*turn_started);
                    }
                    Err(_) => {
                        current_admission.state = ChatPromptDeliveryState::RecoveryRequired;
                        current_admission.failure_phase = Some(ChatRevivePhase::PromptDelivery);
                        current_admission.failure_retryable = outcome
                            .as_ref()
                            .err()
                            .is_some_and(revive_error_is_retryable);
                    }
                }
                current.updated_at = crate::clock::now_rfc3339();
                match hosts.update_if_revision(current.revision, &current).await {
                    Ok(Some(committed)) => {
                        host = committed;
                        break;
                    }
                    Ok(None) => continue,
                    Err(_) => return Err(chat_host_store_unavailable()),
                }
            }
        }
    }

    async fn advance_chat_revive_phase(
        &self,
        hosts: &Arc<dyn ChatHostStore>,
        observed: &ChatHostRecord,
        claim: &str,
        phase: ChatRevivePhase,
    ) -> Result<ChatHostRecord, CoreError> {
        let mut current = hosts
            .get(&observed.id)
            .await
            .map_err(|_| chat_host_store_unavailable())?
            .ok_or(CoreError::ChatUnavailable(ChatUnavailableCause::Retired))?;
        if current.status != ChatHostStatus::Reviving
            || current.last_transition_claim.as_deref() != Some(claim)
        {
            return Err(CoreError::ChatUnavailable(
                ChatUnavailableCause::RevivePending,
            ));
        }
        if current.revive_phase == Some(phase) {
            return Ok(current);
        }
        current.revive_phase = Some(phase);
        current.updated_at = crate::clock::now_rfc3339();
        hosts
            .update_if_revision(current.revision, &current)
            .await
            .map_err(|_| chat_host_store_unavailable())?
            .ok_or(CoreError::ChatUnavailable(
                ChatUnavailableCause::RevivePending,
            ))
    }

    async fn fail_chat_revival(
        &self,
        hosts: &Arc<dyn ChatHostStore>,
        observed: &ChatHostRecord,
        claim: &str,
        failure: ChatRevivalFailure<'_>,
    ) -> Result<(), CoreError> {
        let ChatRevivalFailure {
            phase,
            detail,
            retryable,
            provisioning_phase,
            runtime,
        } = failure;
        let cleanup_pending = match runtime {
            Some((handle, adapter)) => self
                .terminate_chat_replacement(observed, handle, adapter)
                .await
                .is_err(),
            None => false,
        };
        let failures = self
            .mark_recovering_chat_threads_failed(observed, phase, retryable)
            .await;
        {
            let mut entries = self.entries.write().await;
            for session_id in live_session_ids(observed) {
                entries.remove(&session_id);
            }
        }
        loop {
            let Some(mut current) = hosts
                .get(&observed.id)
                .await
                .map_err(|_| chat_host_store_unavailable())?
            else {
                return Ok(());
            };
            if current.last_transition_claim.as_deref() != Some(claim) {
                return Ok(());
            }
            if cleanup_pending {
                current.status = ChatHostStatus::Reviving;
            } else {
                current.status = ChatHostStatus::Paused;
                current.runtime_id = None;
                current.last_transition_claim = None;
                current.transition_started_at = None;
            }
            current.revive_phase = Some(ChatRevivePhase::RecoveryRequired);
            current.revive_failed_threads = failures.clone();
            current.last_failure = Some(crate::ChatHostFailure {
                phase: provisioning_phase.unwrap_or_else(|| revive_provisioning_phase(phase)),
                command_index: None,
                retryable,
                cleanup_pending,
                occurred_at: crate::clock::now_rfc3339(),
            });
            current.last_operator_note = Some(ChatHostOperatorNote {
                code: "recovery_required".into(),
                detail: detail.into(),
                observed_at: crate::clock::now_rfc3339(),
            });
            for admission in &mut current.prompt_admissions {
                if matches!(
                    admission.state,
                    ChatPromptDeliveryState::Queued | ChatPromptDeliveryState::DispatchFenced
                ) {
                    admission.state = ChatPromptDeliveryState::RecoveryRequired;
                    admission.failure_phase = Some(phase);
                    admission.failure_provisioning_phase = provisioning_phase;
                    admission.failure_retryable = retryable;
                }
            }
            current.updated_at = crate::clock::now_rfc3339();
            match hosts.update_if_revision(current.revision, &current).await {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => continue,
                Err(_) => return Err(chat_host_store_unavailable()),
            }
        }
    }

    async fn mark_recovering_chat_threads_failed(
        &self,
        host: &ChatHostRecord,
        phase: ChatRevivePhase,
        retryable: bool,
    ) -> Vec<ChatThreadRestoreFailure> {
        let mut failures = Vec::new();
        for slot in host.threads.iter().filter(|slot| !slot.removed) {
            let Ok(Some(record)) = self.store.get(&slot.session_id).await else {
                continue;
            };
            if record.session.status.is_terminal()
                || record
                    .chat_origin
                    .as_ref()
                    .is_none_or(|origin| origin.host_id != host.id)
            {
                continue;
            }
            let failure = ChatThreadRestoreFailure {
                session_id: slot.session_id.clone(),
                phase,
                retryable,
                occurred_at: crate::clock::now_rfc3339(),
            };
            self.mark_chat_thread_restore_failed(host, &failure).await;
            failures.push(failure);
        }
        failures
    }

    async fn finish_failed_chat_revival_cleanup(
        &self,
        hosts: &Arc<dyn ChatHostStore>,
        observed: &ChatHostRecord,
        claim: &str,
    ) -> Result<(), CoreError> {
        let runtime_id = observed.runtime_id.clone();
        if let Some(runtime_id) = runtime_id.as_ref() {
            let adapter = self.adapters.get(&observed.kernel).ok_or_else(|| {
                CoreError::ChatUnavailable(ChatUnavailableCause::CapabilityUnsupported(
                    "configured chat kernel".into(),
                ))
            })?;
            adapter
                .terminate_if_identity(&RuntimeIdentity {
                    handle: RuntimeHandle(runtime_id.clone()),
                    session: observed.id.0.clone(),
                    generation: Some(generation_for(observed)),
                })
                .await
                .map_err(adapter_error)?;
        }
        loop {
            let mut current = hosts
                .get(&observed.id)
                .await
                .map_err(|_| chat_host_store_unavailable())?
                .ok_or(CoreError::ChatUnavailable(ChatUnavailableCause::Retired))?;
            if current.status != ChatHostStatus::Reviving
                || current.last_transition_claim.as_deref() != Some(claim)
                || current.revive_phase != Some(ChatRevivePhase::RecoveryRequired)
                || current.runtime_id != runtime_id
            {
                return Ok(());
            }
            current.status = ChatHostStatus::Paused;
            current.runtime_id = None;
            current.last_transition_claim = None;
            current.transition_started_at = None;
            if let Some(failure) = current.last_failure.as_mut() {
                failure.cleanup_pending = false;
            }
            current.updated_at = crate::clock::now_rfc3339();
            match hosts.update_if_revision(current.revision, &current).await {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => continue,
                Err(_) => return Err(chat_host_store_unavailable()),
            }
        }
    }

    async fn terminate_chat_replacement(
        &self,
        host: &ChatHostRecord,
        handle: &RuntimeHandle,
        adapter: &Arc<dyn KernelAdapter>,
    ) -> Result<(), AdapterError> {
        adapter
            .terminate_if_identity(&RuntimeIdentity {
                handle: handle.clone(),
                session: host.id.0.clone(),
                generation: Some(generation_for(host)),
            })
            .await
    }
}

fn prune_prompt_admissions(host: &mut ChatHostRecord) {
    let now = crate::seconds_from_rfc3339(&crate::clock::now_rfc3339()).unwrap_or(u64::MAX);
    host.prompt_admissions.retain(|admission| {
        !matches!(
            admission.state,
            ChatPromptDeliveryState::Delivered | ChatPromptDeliveryState::RecoveryRequired
        ) || crate::seconds_from_rfc3339(&admission.admitted_at)
            .is_none_or(|admitted| now < admitted.saturating_add(ADMISSION_RETENTION_SECONDS))
    });
}

fn next_chat_checkpoint_version(host: &ChatHostRecord) -> Option<u64> {
    host.checkpoint_sequence
        .max(
            host.checkpoint
                .as_ref()
                .map_or(0, |checkpoint| checkpoint.checkpoint_version),
        )
        .checked_add(1)
}

fn valid_setup(setup: Option<&WorkspaceSetupReceipt>) -> bool {
    setup.is_some_and(|setup| setup.steps.iter().all(|step| step.exit_code == 0))
}

fn account_paused_image_drift(
    host: &mut ChatHostRecord,
    target_version: Option<&str>,
) -> Result<(), CoreError> {
    let Some(target_version) = target_version else {
        return Ok(());
    };
    if host.runtime_version.as_deref() == Some(target_version) {
        return Ok(());
    }
    let Some(captured_at) = host
        .checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.captured_at.clone())
    else {
        return Ok(());
    };
    if host.rotated_at.as_deref() != Some(captured_at.as_str()) {
        host.rotations = host
            .rotations
            .checked_add(1)
            .ok_or_else(|| CoreError::InvalidRequest("chat rotation count exhausted".into()))?;
        host.rotated_at = Some(captured_at);
    }
    Ok(())
}

fn revive_provisioning_phase(phase: ChatRevivePhase) -> ChatProvisioningPhase {
    match phase {
        ChatRevivePhase::RuntimeProvisioning => ChatProvisioningPhase::Runtime,
        ChatRevivePhase::WorkspaceSetup => ChatProvisioningPhase::WorkspaceSetup,
        ChatRevivePhase::CheckpointVerification
        | ChatRevivePhase::ConversationRestore
        | ChatRevivePhase::ThreadRestore
        | ChatRevivePhase::PromptDelivery
        | ChatRevivePhase::RecoveryRequired => ChatProvisioningPhase::RuntimeLost,
    }
}

fn chat_host_store_unavailable() -> CoreError {
    CoreError::ChatUnavailable(ChatUnavailableCause::HostStoreUnavailable)
}

fn chat_restore_failed(phase: ChatRevivePhase, retryable: bool) -> CoreError {
    CoreError::ChatUnavailable(ChatUnavailableCause::RestoreFailed { phase, retryable })
}

fn revive_error_is_retryable(error: &CoreError) -> bool {
    match error {
        CoreError::RuntimeUnavailable(_) => true,
        CoreError::ChatUnavailable(cause) => cause.is_transient(),
        _ => false,
    }
}

fn session_store_error(error: StoreError) -> CoreError {
    CoreError::RuntimeUnavailable(error.to_string())
}

fn adapter_error(error: AdapterError) -> CoreError {
    if error.is_runtime_loss() {
        CoreError::RuntimeUnavailable(error.to_string())
    } else {
        CoreError::RuntimeRefused(error.to_string())
    }
}
