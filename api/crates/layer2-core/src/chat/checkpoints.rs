//! Opportunistic recovery points for quiet shared chat runtimes.

use std::sync::Arc;

use layer3_adapter::{
    KernelAdapter, RuntimeHandle, WorkspaceCheckpointDisposition, WorkspaceCheckpointPackage,
    WorkspaceCheckpointRequest, WorkspaceWriterState,
};

use crate::chat::checkpoint::digest;
use crate::store::{ChatCheckpointStore, ChatHostStore, PageRequest, SessionRecord, StoreError};
use crate::{
    ChatCheckpointConversation, ChatCheckpointRef, ChatHostId, ChatHostListQuery,
    ChatHostOperatorNote, ChatHostRecord, ChatHostStatus, CoreError, SessionId, SessionStatus,
};

pub const CHAT_CHECKPOINT_INTERVAL_SECONDS: u64 = 5 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatCheckpointOutcome {
    NotEligible,
    Busy,
    Refused,
    Complete(ChatCheckpointRef),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChatCheckpointSweep {
    pub candidates: usize,
    pub completed: usize,
    pub skipped: usize,
    pub refused: usize,
}

struct ConversationTarget {
    session_id: SessionId,
    conversation_id: String,
}

/// A verified recovery point whose runtime writer fence is still held.
///
/// Periodic capture reopens it before publication. A pause keeps it closed
/// through durable lifecycle publication and consumes it by fenced teardown.
pub(super) struct HeldChatCheckpoint {
    pub(super) reference: ChatCheckpointRef,
    pub(super) adapter: Arc<dyn KernelAdapter>,
    pub(super) runtime: RuntimeHandle,
    pub(super) writer_resume_token: String,
    pub(super) objects: Arc<dyn ChatCheckpointStore>,
}

impl HeldChatCheckpoint {
    pub(super) async fn resume_writers(&self) -> Result<(), CoreError> {
        self.adapter
            .resume_workspace_writers(&self.runtime, &self.writer_resume_token)
            .await
            .map_err(|_| {
                CoreError::RuntimeUnavailable("chat workspace writers could not be reopened".into())
            })
    }
}

pub(super) struct ChatCaptureRefusal {
    pub(super) detail: &'static str,
    pub(super) writers_reopened: bool,
}

impl crate::SessionService {
    /// Run one bounded active-host pass. Cadence belongs to the binary.
    pub async fn checkpoint_due_chat_hosts(
        &self,
        now: &str,
    ) -> Result<ChatCheckpointSweep, CoreError> {
        if crate::seconds_from_rfc3339(now).is_none() {
            return Err(CoreError::InvalidRequest(
                "chat checkpoint time is not a CMA timestamp".into(),
            ));
        }
        let hosts = self
            .chat_hosts
            .as_ref()
            .ok_or_else(checkpoint_host_store_unavailable)?;
        let page = hosts
            .list_candidates(&ChatHostListQuery::new(
                PageRequest::new(crate::MAX_PAGE_SIZE, None),
                vec![ChatHostStatus::Active],
            ))
            .await
            .map_err(|_| checkpoint_host_store_unavailable())?;
        let mut sweep = ChatCheckpointSweep {
            candidates: page.items.len(),
            ..ChatCheckpointSweep::default()
        };
        for host in page.items {
            match self.checkpoint_chat_host(&host.id, now).await {
                Ok(ChatCheckpointOutcome::Complete(_)) => sweep.completed += 1,
                Ok(ChatCheckpointOutcome::Refused) => sweep.refused += 1,
                Ok(ChatCheckpointOutcome::NotEligible | ChatCheckpointOutcome::Busy) => {
                    sweep.skipped += 1;
                }
                Err(error) => {
                    sweep.refused += 1;
                    tracing::warn!(
                        chat_host = %host.id,
                        %error,
                        "chat checkpoint attempt could not settle its durable note"
                    );
                }
            }
        }
        Ok(sweep)
    }

    /// Capture one active host only when every retained thread is quiet.
    pub async fn checkpoint_chat_host(
        &self,
        host_id: &ChatHostId,
        now: &str,
    ) -> Result<ChatCheckpointOutcome, CoreError> {
        let Some(now_seconds) = crate::seconds_from_rfc3339(now) else {
            return Err(CoreError::InvalidRequest(
                "chat checkpoint time is not a CMA timestamp".into(),
            ));
        };
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(checkpoint_host_store_unavailable)?;
        let observed = match hosts
            .get(host_id)
            .await
            .map_err(|_| checkpoint_host_store_unavailable())?
        {
            Some(host) => host,
            None => return Ok(ChatCheckpointOutcome::NotEligible),
        };
        if !chat_checkpoint_due(&observed, now_seconds) {
            return Ok(ChatCheckpointOutcome::NotEligible);
        }

        let initial_sessions = live_session_ids(&observed);
        if initial_sessions.is_empty() {
            return Ok(ChatCheckpointOutcome::NotEligible);
        }
        let mut input_fences = Vec::with_capacity(initial_sessions.len());
        for session_id in &initial_sessions {
            let Ok(fence) = self.input_gates.gate(session_id).try_write_owned() else {
                return Ok(ChatCheckpointOutcome::Busy);
            };
            input_fences.push(fence);
        }
        let _input_fences = input_fences;
        let Some(_owner_guard) = self.chat_host_guards.try_lock(&observed.owner).await else {
            return Ok(ChatCheckpointOutcome::Busy);
        };

        let current = match hosts
            .get(host_id)
            .await
            .map_err(|_| checkpoint_host_store_unavailable())?
        {
            Some(host) => host,
            None => return Ok(ChatCheckpointOutcome::NotEligible),
        };
        if !chat_checkpoint_due(&current, now_seconds)
            || current.owner != observed.owner
            || live_session_ids(&current) != initial_sessions
        {
            return Ok(ChatCheckpointOutcome::Busy);
        }
        match self.quiet_chat_thread_records(&current).await? {
            Some(_) => {}
            None => return Ok(ChatCheckpointOutcome::NotEligible),
        }

        let claim = format!("chcp_{}", uuid::Uuid::new_v4().simple());
        let version = current
            .checkpoint_sequence
            .max(
                current
                    .checkpoint
                    .as_ref()
                    .map_or(0, |checkpoint| checkpoint.checkpoint_version),
            )
            .checked_add(1);
        let mut claimed = current.clone();
        claimed.last_transition_claim = Some(claim.clone());
        claimed.transition_started_at = Some(now.to_string());
        if let Some(version) = version {
            claimed.checkpoint_sequence = version;
        }
        claimed.updated_at = now.to_string();
        let Some(claimed) = hosts
            .update_if_revision(current.revision, &claimed)
            .await
            .map_err(|_| checkpoint_host_store_unavailable())?
        else {
            return Ok(ChatCheckpointOutcome::Busy);
        };

        let records = match self.quiet_chat_thread_records(&claimed).await {
            Ok(Some(records)) => records,
            Ok(None) => {
                self.release_chat_checkpoint_claim(&hosts, host_id, &claim, now, None)
                    .await?;
                return Ok(ChatCheckpointOutcome::NotEligible);
            }
            Err(_) => {
                return self
                    .refuse_chat_checkpoint(
                        &hosts,
                        host_id,
                        &claim,
                        now,
                        "The durable chat thread set could not be verified.",
                    )
                    .await;
            }
        };
        let version = match version {
            Some(version) => version,
            None => {
                return self
                    .refuse_chat_checkpoint(
                        &hosts,
                        host_id,
                        &claim,
                        now,
                        "The chat checkpoint version is exhausted.",
                    )
                    .await;
            }
        };
        let capture = match self
            .capture_claimed_chat_checkpoint(&claimed, &records, version, now)
            .await
        {
            Ok(capture) => capture,
            Err(refusal) => {
                return self
                    .refuse_chat_checkpoint(&hosts, host_id, &claim, now, refusal.detail)
                    .await;
            }
        };

        // Input and owner gates are still held, so reopening writers cannot
        // admit a new turn or worktree before the host CAS below.
        if capture.resume_writers().await.is_err() {
            return self
                .refuse_chat_checkpoint(
                    &hosts,
                    host_id,
                    &claim,
                    now,
                    "Workspace writer resumption could not be verified.",
                )
                .await;
        }

        let mut committed = claimed.clone();
        committed.checkpoint = Some(capture.reference.clone());
        committed.last_transition_claim = None;
        committed.transition_started_at = None;
        committed.last_operator_note = None;
        committed.updated_at = now.to_string();
        let committed = match hosts.update_if_revision(claimed.revision, &committed).await {
            Ok(committed) => committed,
            Err(_) => {
                return self
                    .refuse_chat_checkpoint(
                        &hosts,
                        host_id,
                        &claim,
                        now,
                        "The host checkpoint could not be durably published.",
                    )
                    .await;
            }
        };
        if committed.is_none() {
            return self
                .refuse_chat_checkpoint(
                    &hosts,
                    host_id,
                    &claim,
                    now,
                    "The host changed before checkpoint publication.",
                )
                .await;
        }

        self.prune_previous_chat_checkpoint(&capture.objects, host_id, &capture.reference)
            .await;
        Ok(ChatCheckpointOutcome::Complete(capture.reference))
    }

    pub(super) async fn capture_claimed_chat_checkpoint(
        &self,
        claimed: &ChatHostRecord,
        records: &[SessionRecord],
        version: u64,
        now: &str,
    ) -> Result<HeldChatCheckpoint, ChatCaptureRefusal> {
        let targets =
            self.chat_conversation_targets(records)
                .await
                .map_err(|_| ChatCaptureRefusal {
                    detail: "A durable conversation identity could not be resolved.",
                    writers_reopened: true,
                })?;
        let authority = claimed
            .workspace_authority
            .as_ref()
            .ok_or(ChatCaptureRefusal {
                detail: "The runtime has no verified workspace source authority.",
                writers_reopened: true,
            })?;
        let runtime_id = claimed.runtime_id.as_ref().ok_or(ChatCaptureRefusal {
            detail: "The active chat host has no runtime identity.",
            writers_reopened: true,
        })?;
        let adapter = self
            .adapters
            .get(&claimed.kernel)
            .cloned()
            .ok_or(ChatCaptureRefusal {
                detail: "The configured chat checkpoint adapter is unavailable.",
                writers_reopened: true,
            })?;
        let objects = self
            .chat_checkpoints
            .as_ref()
            .cloned()
            .ok_or(ChatCaptureRefusal {
                detail: "Durable chat checkpoint storage is not configured.",
                writers_reopened: true,
            })?;
        let request = WorkspaceCheckpointRequest {
            epoch: authority.workspace_epoch,
            version,
            source_revision: authority.source_revision.clone(),
            baseline_manifest_digest: authority.baseline_manifest_digest.clone(),
            include_git_metadata: true,
            disposition: WorkspaceCheckpointDisposition::HoldForTeardown,
        };
        let runtime = RuntimeHandle(runtime_id.clone());
        let workspace = adapter
            .checkpoint_workspace(&runtime, &request)
            .await
            .map_err(|_| ChatCaptureRefusal {
                detail: "The runtime refused the complete workspace capture.",
                writers_reopened: true,
            })?;
        let writer_resume_token = match validate_held_chat_workspace(&request, &workspace) {
            Ok(token) => token,
            Err(_) => {
                let writers_reopened = match workspace.writer_resume_token.as_deref() {
                    Some(token) => adapter
                        .resume_workspace_writers(&runtime, token)
                        .await
                        .is_ok(),
                    None => false,
                };
                return Err(ChatCaptureRefusal {
                    detail: "The runtime returned an invalid workspace-fence receipt.",
                    writers_reopened,
                });
            }
        };

        let conversations = match self
            .capture_chat_conversations(&adapter, &runtime, &targets)
            .await
        {
            Ok(conversations) => conversations,
            Err(_) => {
                let writers_reopened = adapter
                    .resume_workspace_writers(&runtime, &writer_resume_token)
                    .await
                    .is_ok();
                let detail = if writers_reopened {
                    "One or more chat conversations could not be captured."
                } else {
                    "Workspace writer resumption could not be verified."
                };
                return Err(ChatCaptureRefusal {
                    detail,
                    writers_reopened,
                });
            }
        };
        let reference = match self
            .stage_and_verify_chat_checkpoint(&objects, claimed, &workspace, conversations, now)
            .await
        {
            Ok(reference) => reference,
            Err(_) => {
                let writers_reopened = adapter
                    .resume_workspace_writers(&runtime, &writer_resume_token)
                    .await
                    .is_ok();
                let detail = if writers_reopened {
                    "The captured chat checkpoint could not be durably verified."
                } else {
                    "Workspace writer resumption could not be verified."
                };
                return Err(ChatCaptureRefusal {
                    detail,
                    writers_reopened,
                });
            }
        };
        Ok(HeldChatCheckpoint {
            reference,
            adapter,
            runtime,
            writer_resume_token,
            objects,
        })
    }

    async fn quiet_chat_thread_records(
        &self,
        host: &ChatHostRecord,
    ) -> Result<Option<Vec<SessionRecord>>, CoreError> {
        let records = self.chat_thread_records(host).await?;
        if records
            .iter()
            .any(|record| record.session.status == SessionStatus::Running)
        {
            return Ok(None);
        }
        Ok(Some(records))
    }

    pub(super) async fn chat_thread_records(
        &self,
        host: &ChatHostRecord,
    ) -> Result<Vec<SessionRecord>, CoreError> {
        let runtime_id = host.runtime_id.as_deref().ok_or_else(|| {
            CoreError::RuntimeUnavailable("the active chat host has no runtime".into())
        })?;
        let mut records = Vec::new();
        for slot in host.threads.iter().filter(|slot| !slot.removed) {
            let record = self
                .store
                .get(&slot.session_id)
                .await
                .map_err(checkpoint_session_store_error)?
                .ok_or_else(|| {
                    CoreError::RuntimeUnavailable(
                        "a live chat host slot has no durable session".into(),
                    )
                })?;
            let matches = record.session.owner == host.owner
                && record.session.kernel == host.kernel
                && record.runtime_id.as_deref() == Some(runtime_id)
                && record.chat_origin.as_ref().is_some_and(|origin| {
                    origin.host_id == host.id
                        && origin.worktree_name == slot.worktree_name
                        && origin.worktree_path == slot.worktree_path
                });
            if !matches {
                return Err(CoreError::RuntimeUnavailable(
                    "a live chat thread no longer matches its host authority".into(),
                ));
            }
            records.push(record);
        }
        records.sort_by(|left, right| left.session.id.as_str().cmp(right.session.id.as_str()));
        Ok(records)
    }

    async fn chat_conversation_targets(
        &self,
        records: &[SessionRecord],
    ) -> Result<Vec<ConversationTarget>, StoreError> {
        let mut targets = Vec::with_capacity(records.len());
        for record in records {
            let conversation_id = self
                .readoption_history(record)
                .await?
                .conversation_id
                .filter(|conversation_id| !conversation_id.trim().is_empty())
                .ok_or_else(|| {
                    StoreError::Malformed(
                        "a live chat thread has no durable conversation identity".into(),
                    )
                })?;
            targets.push(ConversationTarget {
                session_id: record.session.id.clone(),
                conversation_id,
            });
        }
        Ok(targets)
    }

    async fn capture_chat_conversations(
        &self,
        adapter: &Arc<dyn KernelAdapter>,
        runtime: &RuntimeHandle,
        targets: &[ConversationTarget],
    ) -> Result<Vec<ChatCheckpointConversation>, CoreError> {
        let mut captured = Vec::with_capacity(targets.len());
        for target in targets {
            let snapshot = adapter
                .snapshot_conversation(runtime, &target.conversation_id)
                .await
                .map_err(|_| CoreError::RuntimeRefused("chat conversation capture failed".into()))?
                .ok_or_else(|| {
                    CoreError::RuntimeRefused(
                        "chat conversation capture returned no snapshot".into(),
                    )
                })?;
            let bytes = u64::try_from(snapshot.len()).unwrap_or(u64::MAX);
            if bytes == 0 || bytes > crate::MAX_CHAT_CONVERSATION_SNAPSHOT_BYTES {
                return Err(CoreError::RuntimeRefused(
                    "chat conversation capture is outside its byte bound".into(),
                ));
            }
            captured.push(ChatCheckpointConversation {
                session_id: target.session_id.clone(),
                object_reference: String::new(),
                conversation_id: target.conversation_id.clone(),
                snapshot,
            });
        }
        Ok(captured)
    }

    async fn stage_and_verify_chat_checkpoint(
        &self,
        objects: &Arc<dyn ChatCheckpointStore>,
        host: &ChatHostRecord,
        workspace: &WorkspaceCheckpointPackage,
        mut conversations: Vec<ChatCheckpointConversation>,
        now: &str,
    ) -> Result<ChatCheckpointRef, StoreError> {
        let previous = host.checkpoint.as_ref();
        for conversation in &mut conversations {
            let previous_sha = previous.and_then(|checkpoint| {
                checkpoint
                    .conversations
                    .iter()
                    .find(|prior| prior.session_id == conversation.session_id)
                    .map(|prior| prior.sha256.as_str())
            });
            let expected_sha = digest(&conversation.snapshot);
            let expected_bytes = u64::try_from(conversation.snapshot.len()).unwrap_or(u64::MAX);
            let receipt = objects
                .stage_conversation(
                    &conversation.session_id,
                    previous_sha,
                    conversation.snapshot.clone(),
                )
                .await?;
            let expected_reference =
                format!("sessions/{}/handover.snapshot", conversation.session_id.0);
            if receipt.object_reference != expected_reference
                || receipt.sha256 != expected_sha
                || receipt.bytes != expected_bytes
            {
                return Err(StoreError::Malformed(
                    "chat conversation storage receipt changed its sealed identity".into(),
                ));
            }
            conversation.object_reference = receipt.object_reference;
        }

        let workspace_receipt = objects
            .stage_workspace(
                &host.id,
                previous.map(|checkpoint| checkpoint.workspace_archive_sha256.as_str()),
                workspace.archive.clone(),
            )
            .await?;
        let expected_workspace_reference = format!("chat-hosts/{}/checkpoint-v1", host.id.0);
        if workspace_receipt.object_reference != expected_workspace_reference
            || workspace_receipt.sha256 != workspace.archive_sha256
            || workspace_receipt.bytes != u64::try_from(workspace.archive.len()).unwrap_or(u64::MAX)
        {
            return Err(StoreError::Malformed(
                "chat workspace storage receipt changed its sealed identity".into(),
            ));
        }
        let reference = ChatCheckpointRef::seal(
            workspace_receipt.object_reference,
            workspace,
            conversations.clone(),
            now,
        )
        .map_err(StoreError::Malformed)?;

        let workspace_readback = objects
            .read_workspace(
                &host.id,
                &reference.workspace_object_reference,
                &reference.workspace_archive_sha256,
            )
            .await?
            .ok_or_else(|| StoreError::Unavailable("chat workspace readback is missing".into()))?;
        if workspace_readback.len() != workspace.archive.len()
            || digest(&workspace_readback) != reference.workspace_archive_sha256
        {
            return Err(StoreError::Malformed(
                "chat workspace readback failed its size or hash".into(),
            ));
        }
        let mut verified_conversations = Vec::with_capacity(conversations.len());
        for conversation in conversations {
            let expected = reference
                .conversations
                .iter()
                .find(|expected| expected.session_id == conversation.session_id)
                .expect("the reference was sealed from this conversation set");
            let snapshot = objects
                .read_conversation(
                    &conversation.session_id,
                    &expected.object_reference,
                    &expected.sha256,
                )
                .await?
                .ok_or_else(|| {
                    StoreError::Unavailable("chat conversation readback is missing".into())
                })?;
            if snapshot.len() != conversation.snapshot.len() || digest(&snapshot) != expected.sha256
            {
                return Err(StoreError::Malformed(
                    "chat conversation readback failed its size or hash".into(),
                ));
            }
            verified_conversations.push(ChatCheckpointConversation {
                snapshot,
                ..conversation
            });
        }
        let mut verified_workspace = workspace.clone();
        verified_workspace.archive = workspace_readback;
        reference
            .validate_bytes(&verified_workspace, &verified_conversations)
            .map_err(StoreError::Malformed)?;
        Ok(reference)
    }

    pub(super) async fn prune_previous_chat_checkpoint(
        &self,
        objects: &Arc<dyn ChatCheckpointStore>,
        host_id: &ChatHostId,
        checkpoint: &ChatCheckpointRef,
    ) {
        if let Err(error) = objects
            .prune_workspace(host_id, &checkpoint.workspace_archive_sha256)
            .await
        {
            tracing::warn!(chat_host = %host_id, %error, "old chat workspace generation remains retained");
        }
        for conversation in &checkpoint.conversations {
            if let Err(error) = objects
                .prune_conversation(&conversation.session_id, &conversation.sha256)
                .await
            {
                tracing::warn!(
                    chat_host = %host_id,
                    session_id = %conversation.session_id,
                    %error,
                    "old chat conversation generation remains retained"
                );
            }
        }
    }

    async fn refuse_chat_checkpoint(
        &self,
        hosts: &Arc<dyn ChatHostStore>,
        host_id: &ChatHostId,
        claim: &str,
        now: &str,
        detail: &'static str,
    ) -> Result<ChatCheckpointOutcome, CoreError> {
        self.release_chat_checkpoint_claim(hosts, host_id, claim, now, Some(detail))
            .await?;
        Ok(ChatCheckpointOutcome::Refused)
    }

    async fn release_chat_checkpoint_claim(
        &self,
        hosts: &Arc<dyn ChatHostStore>,
        host_id: &ChatHostId,
        claim: &str,
        now: &str,
        refusal: Option<&'static str>,
    ) -> Result<(), CoreError> {
        loop {
            let Some(current) = hosts
                .get(host_id)
                .await
                .map_err(|_| checkpoint_host_store_unavailable())?
            else {
                return Ok(());
            };
            if current.last_transition_claim.as_deref() != Some(claim) {
                return Ok(());
            }
            let mut replacement = current.clone();
            replacement.last_transition_claim = None;
            replacement.transition_started_at = None;
            if let Some(detail) = refusal {
                replacement.last_operator_note = Some(ChatHostOperatorNote {
                    code: "checkpoint_refused".into(),
                    detail: detail.into(),
                    observed_at: now.into(),
                });
            }
            replacement.updated_at = now.into();
            if hosts
                .update_if_revision(current.revision, &replacement)
                .await
                .map_err(|_| checkpoint_host_store_unavailable())?
                .is_some()
            {
                return Ok(());
            }
        }
    }
}

fn chat_checkpoint_due(host: &ChatHostRecord, now_seconds: u64) -> bool {
    if host.status != ChatHostStatus::Active
        || host.last_transition_claim.is_some()
        || host.runtime_id.is_none()
    {
        return false;
    }
    host.checkpoint.as_ref().is_none_or(|checkpoint| {
        crate::seconds_from_rfc3339(&checkpoint.captured_at).is_some_and(|captured| {
            now_seconds >= captured.saturating_add(CHAT_CHECKPOINT_INTERVAL_SECONDS)
        })
    })
}

pub(super) fn live_session_ids(host: &ChatHostRecord) -> Vec<SessionId> {
    let mut sessions = host
        .threads
        .iter()
        .filter(|thread| !thread.removed)
        .map(|thread| thread.session_id.clone())
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    sessions
}

fn validate_held_chat_workspace(
    request: &WorkspaceCheckpointRequest,
    workspace: &WorkspaceCheckpointPackage,
) -> Result<String, CoreError> {
    crate::workspace_checkpoint::validate_workspace_checkpoint_package(request, workspace)?;
    if workspace.writer_state != WorkspaceWriterState::HeldForTeardown {
        return Err(CoreError::RuntimeRefused(
            "chat workspace capture did not retain its writer fence".into(),
        ));
    }
    workspace
        .writer_resume_token
        .clone()
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| {
            CoreError::RuntimeUnavailable(
                "chat workspace capture omitted its writer resume token".into(),
            )
        })
}

fn checkpoint_host_store_unavailable() -> CoreError {
    CoreError::ChatUnavailable(crate::ChatUnavailableCause::HostStoreUnavailable)
}

fn checkpoint_session_store_error(error: StoreError) -> CoreError {
    CoreError::RuntimeUnavailable(error.to_string())
}
