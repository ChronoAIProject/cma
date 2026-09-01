//! Durable inactivity decisions and shared-host pause choreography.

use std::sync::Arc;

use layer3_adapter::RuntimeIdentity;

use super::checkpoints::{live_session_ids, ChatCaptureRefusal};
use super::host::generation_for;
use crate::conversation_log::pending_approvals_from;
use crate::event::{Event, EventKind, StoredEventKind};
use crate::session::{RecoveryState, SessionStatus, StopReason};
use crate::store::{ChatHostStore, Page, PageRequest, SessionRecord, StoreError};
use crate::{
    Caller, Capability, ChatCheckpointRef, ChatHostId, ChatHostListQuery, ChatHostOperatorNote,
    ChatHostRecord, ChatHostStatus, CoreError,
};

/// A refused capture is not retried on every one-minute scheduler heartbeat.
pub const CHAT_PAUSE_REFUSAL_BACKOFF_SECONDS: u64 = 5 * 60;

const CHAT_RETIREMENT_CLAIM_PREFIX: &str = "chrt_";
const CHAT_ADMIN_RETIREMENT_DETAIL: &str =
    "Administrative chat-host retirement revoked this thread.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatPauseOutcome {
    NotEligible,
    Busy,
    Refused,
    Complete(ChatCheckpointRef),
}

impl crate::SessionService {
    /// Return one bounded page of active hosts proven quiet at one instant.
    ///
    /// The continuation belongs to the unfiltered active-host page. Preserving
    /// it even when no row is quiet prevents a noisy first page from starving
    /// every host after it.
    pub async fn chat_pause_candidates(
        &self,
        now: &str,
        page: PageRequest,
    ) -> Result<Page<ChatHostRecord>, CoreError> {
        let now_seconds = parse_pause_time(now)?;
        let hosts = self
            .chat_hosts
            .as_ref()
            .ok_or_else(pause_host_store_unavailable)?;
        let page = hosts
            .list_candidates(&ChatHostListQuery::new(page, vec![ChatHostStatus::Active]))
            .await
            .map_err(|_| pause_host_store_unavailable())?;
        let mut candidates = Vec::with_capacity(page.items.len());
        for host in page.items {
            if pause_claimable(&host, now_seconds)
                && self
                    .quiet_chat_thread_records_at(&host, now_seconds)
                    .await?
                    .is_some()
            {
                candidates.push(host);
            }
        }
        Ok(Page {
            items: candidates,
            next: page.next,
        })
    }

    /// Pause one host only while the same durable quiet boundary still holds.
    pub async fn pause_chat_host(
        &self,
        host_id: &ChatHostId,
        now: &str,
    ) -> Result<ChatPauseOutcome, CoreError> {
        let now_seconds = parse_pause_time(now)?;
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(pause_host_store_unavailable)?;
        let observed = match hosts
            .get(host_id)
            .await
            .map_err(|_| pause_host_store_unavailable())?
        {
            Some(host) => host,
            None => return Ok(ChatPauseOutcome::NotEligible),
        };
        if !pause_claimable(&observed, now_seconds)
            || self
                .quiet_chat_thread_records_at(&observed, now_seconds)
                .await?
                .is_none()
        {
            return Ok(ChatPauseOutcome::NotEligible);
        }

        let session_ids = live_session_ids(&observed);
        let version = next_checkpoint_version(&observed);
        let claim = format!("chpa_{}", uuid::Uuid::new_v4().simple());
        let mut proposal = observed.clone();
        proposal.status = ChatHostStatus::Pausing;
        proposal.last_transition_claim = Some(claim.clone());
        proposal.transition_started_at = Some(now.to_string());
        if let Some(version) = version {
            proposal.checkpoint_sequence = version;
        }
        proposal.updated_at = now.to_string();
        let Some(claimed) = hosts
            .update_if_revision(observed.revision, &proposal)
            .await
            .map_err(|_| pause_host_store_unavailable())?
        else {
            return Ok(ChatPauseOutcome::Busy);
        };

        // Every local input path takes the read side. Acquiring all write sides
        // after the durable claim lets an already-admitted turn finish, while a
        // later prompt observes `pausing` and cannot reach the runtime.
        let mut input_fences = Vec::with_capacity(session_ids.len());
        for session_id in &session_ids {
            input_fences.push(self.input_gates.gate(session_id).write_owned().await);
        }
        let owner_guard = self.chat_host_guards.lock(&claimed.owner).await;

        let current = match hosts
            .get(host_id)
            .await
            .map_err(|_| pause_host_store_unavailable())?
        {
            Some(current)
                if current.status == ChatHostStatus::Pausing
                    && current.last_transition_claim.as_deref() == Some(claim.as_str())
                    && current.owner == claimed.owner
                    && current.runtime_id == claimed.runtime_id
                    && live_session_ids(&current) == session_ids =>
            {
                current
            }
            Some(current) if current.last_transition_claim.as_deref() == Some(claim.as_str()) => {
                self.settle_chat_pause_claim(&hosts, host_id, &claim, now, true, None)
                    .await?;
                return Ok(ChatPauseOutcome::Busy);
            }
            _ => return Ok(ChatPauseOutcome::Busy),
        };
        let records = match self
            .quiet_chat_thread_records_at(&current, now_seconds)
            .await
        {
            Ok(Some(records)) => records,
            Ok(None) => {
                self.settle_chat_pause_claim(&hosts, host_id, &claim, now, true, None)
                    .await?;
                return Ok(ChatPauseOutcome::NotEligible);
            }
            Err(_) => {
                self.settle_chat_pause_claim(
                    &hosts,
                    host_id,
                    &claim,
                    now,
                    true,
                    Some("The durable chat thread set could not be verified."),
                )
                .await?;
                return Ok(ChatPauseOutcome::Refused);
            }
        };
        let Some(version) = version else {
            self.settle_chat_pause_claim(
                &hosts,
                host_id,
                &claim,
                now,
                true,
                Some("The chat checkpoint version is exhausted."),
            )
            .await?;
            return Ok(ChatPauseOutcome::Refused);
        };
        let capture = match self
            .capture_claimed_chat_checkpoint(&current, &records, version, now)
            .await
        {
            Ok(capture) => capture,
            Err(ChatCaptureRefusal {
                detail,
                writers_reopened,
            }) => {
                self.settle_chat_pause_claim(
                    &hosts,
                    host_id,
                    &claim,
                    now,
                    writers_reopened,
                    Some(detail),
                )
                .await?;
                return Ok(ChatPauseOutcome::Refused);
            }
        };

        let notice = chat_pause_notice(
            current
                .pinned_settings
                .settings
                .inactivity
                .pause_after_seconds,
        );
        let mut parked = Vec::with_capacity(records.len());
        for record in records {
            match self.park_chat_thread(record.clone(), &notice).await {
                Ok(()) => parked.push(record),
                Err(_) => {
                    let writers_reopened = capture.resume_writers().await.is_ok();
                    let threads_restored = self.restore_parked_chat_threads(&parked).await;
                    self.settle_chat_pause_claim(
                        &hosts,
                        host_id,
                        &claim,
                        now,
                        writers_reopened && threads_restored,
                        Some("The chat threads could not be durably parked."),
                    )
                    .await?;
                    return Ok(ChatPauseOutcome::Refused);
                }
            }
        }

        let published = match self
            .publish_chat_pause(&hosts, &current, &claim, &capture.reference, now)
            .await
        {
            Ok(Some(published)) => published,
            Ok(None) | Err(_) => {
                let writers_reopened = capture.resume_writers().await.is_ok();
                let threads_restored = self.restore_parked_chat_threads(&parked).await;
                self.settle_chat_pause_claim(
                    &hosts,
                    host_id,
                    &claim,
                    now,
                    writers_reopened && threads_restored,
                    Some("The paused host state could not be durably published."),
                )
                .await?;
                return Ok(ChatPauseOutcome::Refused);
            }
        };

        self.prune_previous_chat_checkpoint(&capture.objects, host_id, &capture.reference)
            .await;
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
            let threads_restored = self.restore_parked_chat_threads(&parked).await;
            self.settle_chat_pause_claim(
                &hosts,
                host_id,
                &claim,
                now,
                writers_reopened && threads_restored,
                Some("The verified chat runtime could not be fenced-deleted."),
            )
            .await?;
            return Ok(ChatPauseOutcome::Refused);
        }

        {
            let mut entries = self.entries.write().await;
            for session_id in &session_ids {
                entries.remove(session_id);
            }
        }
        self.finish_chat_pause(&hosts, host_id, &claim, &capture.reference, now)
            .await?;
        drop(owner_guard);
        drop(input_fences);
        Ok(ChatPauseOutcome::Complete(capture.reference))
    }

    /// Retire one shared host without discarding its thread histories or its
    /// last verified checkpoint objects.
    pub async fn retire_chat_host(
        &self,
        caller: &Caller,
        host_id: &ChatHostId,
    ) -> Result<ChatHostRecord, CoreError> {
        caller
            .authorization
            .require(Capability::ControlAnySession)?;
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(pause_host_store_unavailable)?;

        loop {
            let mut host = hosts
                .get(host_id)
                .await
                .map_err(|_| pause_host_store_unavailable())?
                .ok_or(CoreError::SessionNotFound)?;
            if host.status == ChatHostStatus::Retired {
                return Ok(host);
            }
            if host
                .last_transition_claim
                .as_deref()
                .is_some_and(|claim| claim.starts_with(CHAT_RETIREMENT_CLAIM_PREFIX))
            {
                self.resume_chat_retirement_claim(host_id).await?;
                continue;
            }
            if host.last_transition_claim.is_some()
                || !matches!(host.status, ChatHostStatus::Active | ChatHostStatus::Paused)
            {
                return Err(CoreError::InvalidState {
                    detail: Some("chat host retirement requires an active or paused host".into()),
                });
            }

            let now = crate::clock::now_rfc3339();
            host.last_transition_claim = Some(format!(
                "{CHAT_RETIREMENT_CLAIM_PREFIX}{}",
                uuid::Uuid::new_v4().simple()
            ));
            host.transition_started_at = Some(now.clone());
            if host.status == ChatHostStatus::Active {
                let version = next_checkpoint_version(&host).ok_or_else(|| {
                    CoreError::InvalidRequest("chat checkpoint version is exhausted".into())
                })?;
                if host.runtime_id.is_none() || host.threads.iter().all(|thread| thread.removed) {
                    return Err(CoreError::InvalidState {
                        detail: Some(
                            "an active chat host requires a runtime and retained thread before retirement"
                                .into(),
                        ),
                    });
                }
                host.status = ChatHostStatus::Pausing;
                host.checkpoint_sequence = version;
            }
            host.updated_at = now;
            if hosts
                .update_if_revision(host.revision, &host)
                .await
                .map_err(|_| pause_host_store_unavailable())?
                .is_none()
            {
                continue;
            }
            self.resume_chat_retirement_claim(host_id).await?;
        }
    }

    /// Resume the exact administrative retirement claim after either an HTTP
    /// retry or a control-plane replacement.
    pub(crate) async fn resume_chat_retirement_claim(
        &self,
        host_id: &ChatHostId,
    ) -> Result<bool, CoreError> {
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(pause_host_store_unavailable)?;
        let Some(observed) = hosts
            .get(host_id)
            .await
            .map_err(|_| pause_host_store_unavailable())?
            .filter(|host| {
                matches!(
                    host.status,
                    ChatHostStatus::Pausing | ChatHostStatus::Paused
                ) && host
                    .last_transition_claim
                    .as_deref()
                    .is_some_and(|claim| claim.starts_with(CHAT_RETIREMENT_CLAIM_PREFIX))
            })
        else {
            return Ok(false);
        };
        let claim = observed
            .last_transition_claim
            .clone()
            .expect("the retirement predicate proved a claim");
        let session_ids = live_session_ids(&observed);
        let mut input_fences = Vec::with_capacity(session_ids.len());
        for session_id in &session_ids {
            input_fences.push(self.input_gates.gate(session_id).write_owned().await);
        }
        let owner_guard = self.chat_host_guards.lock(&observed.owner).await;
        let Some(current) = hosts
            .get(host_id)
            .await
            .map_err(|_| pause_host_store_unavailable())?
            .filter(|current| {
                matches!(
                    current.status,
                    ChatHostStatus::Pausing | ChatHostStatus::Paused
                ) && current.last_transition_claim.as_deref() == Some(claim.as_str())
                    && current.owner == observed.owner
                    && live_session_ids(current) == session_ids
            })
        else {
            return Ok(false);
        };

        if current.runtime_id.is_none() {
            if current.status != ChatHostStatus::Paused {
                return Err(CoreError::RuntimeUnavailable(
                    "an uncaptured retirement claim lost its chat runtime".into(),
                ));
            }
            let records = self.retirement_thread_records(&current).await?;
            for record in records {
                self.revoke_chat_thread_for_retirement(record).await?;
            }
            self.finish_chat_retirement(&hosts, &current, &claim, None)
                .await?;
            drop(owner_guard);
            drop(input_fences);
            return Ok(true);
        }

        let records = self.chat_thread_records(&current).await?;
        let captured_at = current.transition_started_at.clone().ok_or_else(|| {
            CoreError::RuntimeUnavailable("chat retirement claim has no start time".into())
        })?;
        let capture = match self
            .capture_claimed_chat_checkpoint(
                &current,
                &records,
                current.checkpoint_sequence,
                &captured_at,
            )
            .await
        {
            Ok(capture) => capture,
            Err(ChatCaptureRefusal {
                detail,
                writers_reopened,
            }) => {
                if writers_reopened {
                    self.settle_chat_pause_claim(
                        &hosts,
                        host_id,
                        &claim,
                        &captured_at,
                        true,
                        Some(detail),
                    )
                    .await?;
                }
                return Err(CoreError::RuntimeRefused(detail.into()));
            }
        };
        if current.status == ChatHostStatus::Paused
            && current.checkpoint.as_ref() != Some(&capture.reference)
        {
            let _ = capture.resume_writers().await;
            return Err(CoreError::RuntimeUnavailable(
                "the replayed retirement checkpoint changed its durable reference".into(),
            ));
        }

        for record in records {
            if let Err(error) = self.revoke_chat_thread_for_retirement(record).await {
                let _ = capture.resume_writers().await;
                return Err(error);
            }
        }
        let published = if current.status == ChatHostStatus::Paused {
            current
        } else {
            self.publish_chat_pause(&hosts, &current, &claim, &capture.reference, &captured_at)
                .await?
                .ok_or_else(|| {
                    CoreError::RuntimeUnavailable(
                        "the retirement checkpoint could not be durably published".into(),
                    )
                })?
        };
        // Retirement is a retention boundary: unlike an ordinary lifecycle
        // checkpoint, it must not prune any earlier workspace or conversation
        // generation while replacing the host's published reference.
        capture
            .adapter
            .terminate_if_identity(&RuntimeIdentity {
                handle: capture.runtime.clone(),
                session: published.id.0.clone(),
                generation: Some(generation_for(&published)),
            })
            .await
            .map_err(|_| {
                CoreError::RuntimeUnavailable(
                    "the verified chat runtime could not be fenced-deleted for retirement".into(),
                )
            })?;
        {
            let mut entries = self.entries.write().await;
            for session_id in &session_ids {
                entries.remove(session_id);
            }
        }
        self.finish_chat_retirement(&hosts, &published, &claim, Some(&capture.reference))
            .await?;
        drop(owner_guard);
        drop(input_fences);
        Ok(true)
    }

    /// Finish a retirement whose checkpoint publication is durable and whose
    /// exact runtime absence was established by reconciliation.
    pub(crate) async fn finish_torn_down_chat_retirement(
        &self,
        host_id: &ChatHostId,
    ) -> Result<bool, CoreError> {
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(pause_host_store_unavailable)?;
        let Some(observed) = hosts
            .get(host_id)
            .await
            .map_err(|_| pause_host_store_unavailable())?
            .filter(|host| {
                host.status == ChatHostStatus::Paused
                    && host
                        .last_transition_claim
                        .as_deref()
                        .is_some_and(|claim| claim.starts_with(CHAT_RETIREMENT_CLAIM_PREFIX))
                    && host.checkpoint.as_ref().is_some_and(|checkpoint| {
                        checkpoint.checkpoint_version == host.checkpoint_sequence
                    })
            })
        else {
            return Ok(false);
        };
        let claim = observed
            .last_transition_claim
            .clone()
            .expect("the torn-down retirement predicate proved a claim");
        let session_ids = live_session_ids(&observed);
        let mut input_fences = Vec::with_capacity(session_ids.len());
        for session_id in &session_ids {
            input_fences.push(self.input_gates.gate(session_id).write_owned().await);
        }
        let owner_guard = self.chat_host_guards.lock(&observed.owner).await;
        let current = hosts
            .get(host_id)
            .await
            .map_err(|_| pause_host_store_unavailable())?
            .filter(|current| {
                current.status == ChatHostStatus::Paused
                    && current.last_transition_claim.as_deref() == Some(claim.as_str())
                    && current.checkpoint == observed.checkpoint
                    && live_session_ids(current) == session_ids
            })
            .ok_or_else(|| {
                CoreError::RuntimeUnavailable(
                    "the chat retirement claim changed after runtime teardown".into(),
                )
            })?;
        let records = self.retirement_thread_records(&current).await?;
        for record in records {
            self.revoke_chat_thread_for_retirement(record).await?;
        }
        {
            let mut entries = self.entries.write().await;
            for session_id in &session_ids {
                entries.remove(session_id);
            }
        }
        let checkpoint = current
            .checkpoint
            .clone()
            .expect("the torn-down retirement predicate proved a checkpoint");
        self.finish_chat_retirement(&hosts, &current, &claim, Some(&checkpoint))
            .await?;
        drop(owner_guard);
        drop(input_fences);
        Ok(true)
    }

    async fn retirement_thread_records(
        &self,
        host: &ChatHostRecord,
    ) -> Result<Vec<SessionRecord>, CoreError> {
        let mut records = Vec::new();
        for slot in host.threads.iter().filter(|slot| !slot.removed) {
            let record = self
                .store
                .get(&slot.session_id)
                .await
                .map_err(pause_session_store_error)?
                .ok_or_else(|| {
                    CoreError::RuntimeUnavailable(
                        "a retained chat host slot has no durable session".into(),
                    )
                })?;
            let matches = record.session.owner == host.owner
                && record.session.kernel == host.kernel
                && record.chat_origin.as_ref().is_some_and(|origin| {
                    origin.host_id == host.id
                        && origin.worktree_name == slot.worktree_name
                        && origin.worktree_path == slot.worktree_path
                });
            if !matches {
                return Err(CoreError::RuntimeUnavailable(
                    "a retained chat thread no longer matches its host authority".into(),
                ));
            }
            records.push(record);
        }
        records.sort_by(|left, right| left.session.id.as_str().cmp(right.session.id.as_str()));
        Ok(records)
    }

    async fn revoke_chat_thread_for_retirement(
        &self,
        mut record: SessionRecord,
    ) -> Result<(), CoreError> {
        if record.session.status == SessionStatus::Terminated
            && record.session.stop_reason == Some(StopReason::Revoked)
            && record.session.recovery_state == RecoveryState::Retired
        {
            self.entries.write().await.remove(&record.session.id);
            return Ok(());
        }
        let origin = record.chat_origin.as_mut().ok_or_else(|| {
            CoreError::RuntimeUnavailable("a chat thread lost its host provenance".into())
        })?;
        origin.paused_notice = None;
        record.session.chat = Some(origin.public_projection());
        record.session.status = SessionStatus::Terminated;
        record.session.recovery_state = RecoveryState::Retired;
        record.session.stop_reason = Some(StopReason::Revoked);
        let id = record.session.id.clone();
        self.persist_durably(
            &mut record,
            vec![Event {
                session_id: id.clone(),
                seq: 0,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Terminated,
                    stop_reason: Some(StopReason::Revoked),
                    detail: Some(CHAT_ADMIN_RETIREMENT_DETAIL.into()),
                    capability: None,
                },
                appended_at: String::new(),
            }],
        )
        .await
        .map_err(pause_session_store_error)?;
        self.entries.write().await.remove(&id);
        Ok(())
    }

    async fn finish_chat_retirement(
        &self,
        hosts: &Arc<dyn ChatHostStore>,
        observed: &ChatHostRecord,
        claim: &str,
        checkpoint: Option<&ChatCheckpointRef>,
    ) -> Result<ChatHostRecord, CoreError> {
        if let Some(credentials) = self.credentials.as_ref() {
            credentials
                .release(&observed.id.credential_session_id())
                .await
                .map_err(|_| {
                    CoreError::RuntimeUnavailable(
                        "chat host credential custody could not be retired".into(),
                    )
                })?;
        }
        loop {
            let current = hosts
                .get(&observed.id)
                .await
                .map_err(|_| pause_host_store_unavailable())?
                .ok_or(CoreError::SessionNotFound)?;
            if current.status == ChatHostStatus::Retired {
                return Ok(current);
            }
            if current.last_transition_claim.as_deref() != Some(claim)
                || checkpoint.is_some_and(|checkpoint| {
                    current.status != ChatHostStatus::Paused
                        || current.checkpoint.as_ref() != Some(checkpoint)
                })
            {
                return Err(CoreError::RuntimeUnavailable(
                    "the chat retirement claim changed before publication".into(),
                ));
            }
            let now = crate::clock::now_rfc3339();
            let mut retired = current.clone();
            retired.status = ChatHostStatus::Retired;
            retired.runtime_id = None;
            retired.retired_at = Some(now.clone());
            retired.last_transition_claim = None;
            retired.transition_started_at = None;
            retired.last_failure = None;
            retired.last_operator_note = None;
            retired.revive_cause = None;
            retired.revive_phase = None;
            retired.revive_failed_threads.clear();
            retired.prompt_admissions.clear();
            retired.updated_at = now;
            match hosts
                .update_if_revision(current.revision, &retired)
                .await
                .map_err(|_| pause_host_store_unavailable())?
            {
                Some(committed) => return Ok(committed),
                None => continue,
            }
        }
    }

    /// Resume an inactivity-pause claim after a control-plane replacement.
    /// The claim's already-reserved checkpoint version is replayed verbatim so
    /// Layer 4 either returns the same held fence or refuses recovery.
    pub(crate) async fn resume_chat_pause_claim(
        self: &Arc<Self>,
        host_id: &ChatHostId,
    ) -> Result<bool, CoreError> {
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(pause_host_store_unavailable)?;
        let Some(observed) = hosts
            .get(host_id)
            .await
            .map_err(|_| pause_host_store_unavailable())?
            .filter(|host| {
                matches!(
                    host.status,
                    ChatHostStatus::Pausing | ChatHostStatus::Paused
                ) && host.revive_cause.is_none()
                    && host.last_transition_claim.is_some()
                    && host.runtime_id.is_some()
                    && host.checkpoint_sequence > 0
            })
        else {
            return Ok(false);
        };
        let claim = observed
            .last_transition_claim
            .clone()
            .expect("the resume predicate proved a pause claim");
        let session_ids = live_session_ids(&observed);
        let mut input_fences = Vec::with_capacity(session_ids.len());
        for session_id in &session_ids {
            input_fences.push(self.input_gates.gate(session_id).write_owned().await);
        }
        let owner_guard = self.chat_host_guards.lock(&observed.owner).await;
        let Some(current) = hosts
            .get(host_id)
            .await
            .map_err(|_| pause_host_store_unavailable())?
            .filter(|current| {
                matches!(
                    current.status,
                    ChatHostStatus::Pausing | ChatHostStatus::Paused
                ) && current.revive_cause.is_none()
                    && current.last_transition_claim.as_deref() == Some(claim.as_str())
                    && current.runtime_id == observed.runtime_id
                    && current.checkpoint_sequence == observed.checkpoint_sequence
                    && live_session_ids(current) == session_ids
            })
        else {
            return Ok(false);
        };
        let records = self.chat_thread_records(&current).await?;
        let captured_at = current.transition_started_at.clone().ok_or_else(|| {
            CoreError::RuntimeUnavailable("chat pause claim has no start time".into())
        })?;
        let capture = self
            .capture_claimed_chat_checkpoint(
                &current,
                &records,
                current.checkpoint_sequence,
                &captured_at,
            )
            .await
            .map_err(|refusal| CoreError::RuntimeRefused(refusal.detail.into()))?;
        if current.status == ChatHostStatus::Paused
            && current.checkpoint.as_ref() != Some(&capture.reference)
        {
            let _ = capture.resume_writers().await;
            return Err(CoreError::RuntimeUnavailable(
                "the replayed pause checkpoint changed its durable reference".into(),
            ));
        }

        let notice = chat_pause_notice(
            current
                .pinned_settings
                .settings
                .inactivity
                .pause_after_seconds,
        );
        for record in records {
            if record.session.status == SessionStatus::Idle
                && record.session.stop_reason == Some(StopReason::IdleTimeout)
            {
                continue;
            }
            self.park_chat_thread(record, &notice).await?;
        }
        let published = if current.status == ChatHostStatus::Paused {
            current
        } else {
            self.publish_chat_pause(&hosts, &current, &claim, &capture.reference, &captured_at)
                .await?
                .ok_or(CoreError::ChatUnavailable(
                    crate::ChatUnavailableCause::RevivePending,
                ))?
        };
        capture
            .adapter
            .terminate_if_identity(&RuntimeIdentity {
                handle: capture.runtime.clone(),
                session: published.id.0.clone(),
                generation: Some(generation_for(&published)),
            })
            .await
            .map_err(|_| {
                CoreError::RuntimeUnavailable(
                    "the replayed chat pause could not fence-delete its runtime".into(),
                )
            })?;
        self.prune_previous_chat_checkpoint(&capture.objects, host_id, &capture.reference)
            .await;
        {
            let mut entries = self.entries.write().await;
            for session_id in &session_ids {
                entries.remove(session_id);
            }
        }
        self.finish_chat_pause(&hosts, host_id, &claim, &capture.reference, &captured_at)
            .await?;
        drop(owner_guard);
        drop(input_fences);
        Ok(true)
    }

    /// Complete the last durable pause publication when its old runtime is
    /// already absent. This is safe only for the exact published checkpoint
    /// version; an unpublished capture is never inferred.
    pub(crate) async fn finish_torn_down_chat_pause(
        &self,
        host_id: &ChatHostId,
    ) -> Result<bool, CoreError> {
        let hosts = self
            .chat_hosts
            .clone()
            .ok_or_else(pause_host_store_unavailable)?;
        let Some(current) = hosts
            .get(host_id)
            .await
            .map_err(|_| pause_host_store_unavailable())?
            .filter(|host| {
                host.status == ChatHostStatus::Paused
                    && host.revive_cause.is_none()
                    && host.last_transition_claim.is_some()
                    && host.runtime_id.is_some()
                    && host.checkpoint.as_ref().is_some_and(|checkpoint| {
                        checkpoint.checkpoint_version == host.checkpoint_sequence
                    })
            })
        else {
            return Ok(false);
        };
        let claim = current
            .last_transition_claim
            .as_deref()
            .expect("the torn-down pause predicate proved a claim");
        let checkpoint = current
            .checkpoint
            .as_ref()
            .expect("the torn-down pause predicate proved a checkpoint");
        {
            let mut entries = self.entries.write().await;
            for session_id in live_session_ids(&current) {
                entries.remove(&session_id);
            }
        }
        self.finish_chat_pause(&hosts, host_id, claim, checkpoint, &checkpoint.captured_at)
            .await?;
        Ok(true)
    }

    async fn quiet_chat_thread_records_at(
        &self,
        host: &ChatHostRecord,
        now_seconds: u64,
    ) -> Result<Option<Vec<SessionRecord>>, CoreError> {
        let records = self.chat_thread_records(host).await?;
        if records.is_empty() {
            return Ok(None);
        }
        let pause_after = u64::from(host.pinned_settings.settings.inactivity.pause_after_seconds);
        for record in &records {
            if matches!(
                record.session.status,
                SessionStatus::Running | SessionStatus::Provisioning | SessionStatus::Rotating
            ) {
                return Ok(None);
            }
            let stored = self
                .store
                .events_since(&record.session.id, 0)
                .await
                .map_err(pause_session_store_error)?;
            let mut events = Vec::with_capacity(stored.len());
            for event in stored {
                match event.kind {
                    StoredEventKind::Known(kind) => events.push(Event {
                        session_id: event.session_id,
                        seq: event.seq,
                        kind,
                        appended_at: event.appended_at,
                    }),
                    StoredEventKind::Unknown { tag, .. } => {
                        return Err(CoreError::RuntimeUnavailable(format!(
                            "chat quietness cannot be folded past unknown event {tag}"
                        )));
                    }
                }
            }
            if !pending_approvals_from(&events)
                .map_err(CoreError::RuntimeUnavailable)?
                .pending
                .is_empty()
            {
                return Ok(None);
            }
            let Some(newest) = self
                .store
                .newest_event(&record.session.id)
                .await
                .map_err(pause_session_store_error)?
            else {
                return Ok(None);
            };
            if events.last().map(|event| event.seq) != Some(newest.seq) {
                return Ok(None);
            }
            let Some(appended) = crate::seconds_from_rfc3339(&newest.appended_at) else {
                return Ok(None);
            };
            if now_seconds <= appended.saturating_add(pause_after) {
                return Ok(None);
            }
        }
        Ok(Some(records))
    }

    async fn park_chat_thread(
        &self,
        mut record: SessionRecord,
        notice: &str,
    ) -> Result<(), CoreError> {
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        let origin = record.chat_origin.as_mut().ok_or_else(|| {
            CoreError::RuntimeUnavailable("a chat thread lost its host provenance".into())
        })?;
        origin.paused_notice = Some(notice.to_string());
        record.session.chat = Some(origin.public_projection());
        record.session.status = SessionStatus::Idle;
        record.session.recovery_state = RecoveryState::Idle;
        record.session.stop_reason = Some(StopReason::IdleTimeout);
        let id = record.session.id.clone();
        self.persist_durably(
            &mut record,
            vec![Event {
                session_id: id,
                seq: 0,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Idle,
                    stop_reason: Some(StopReason::IdleTimeout),
                    detail: Some(notice.to_string()),
                    capability: None,
                },
                appended_at: String::new(),
            }],
        )
        .await
        .map_err(pause_session_store_error)
    }

    async fn restore_parked_chat_threads(&self, originals: &[SessionRecord]) -> bool {
        let mut restored = true;
        for original in originals.iter().rev() {
            let result = async {
                let mut current = self.store.get(&original.session.id).await?.ok_or_else(|| {
                    StoreError::SessionDeleted {
                        id: original.session.id.clone(),
                    }
                })?;
                if current.session.status != SessionStatus::Idle
                    || current.session.stop_reason != Some(StopReason::IdleTimeout)
                {
                    return Err(StoreError::RevisionConflict {
                        expected: original.revision,
                        actual: current.revision,
                    });
                }
                current.session.status = original.session.status;
                current.session.recovery_state = original.session.recovery_state;
                current.session.stop_reason = original.session.stop_reason;
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
                                "Automatic idle pause was rolled back before runtime teardown."
                                    .into(),
                            ),
                            capability: None,
                        },
                        appended_at: String::new(),
                    }],
                )
                .await
            }
            .await;
            if let Err(error) = result {
                restored = false;
                tracing::error!(
                    session_id = %original.session.id,
                    %error,
                    "chat pause rollback could not restore a parked thread"
                );
            }
        }
        restored
    }

    async fn publish_chat_pause(
        &self,
        hosts: &Arc<dyn ChatHostStore>,
        claimed: &ChatHostRecord,
        claim: &str,
        checkpoint: &ChatCheckpointRef,
        now: &str,
    ) -> Result<Option<ChatHostRecord>, CoreError> {
        let mut paused = claimed.clone();
        paused.status = ChatHostStatus::Paused;
        paused.checkpoint = Some(checkpoint.clone());
        paused.paused_at = Some(now.to_string());
        paused.last_operator_note = None;
        paused.updated_at = now.to_string();
        match hosts.update_if_revision(claimed.revision, &paused).await {
            Ok(Some(committed)) => Ok(Some(committed)),
            Ok(None) | Err(_) => {
                let current = hosts
                    .get(&claimed.id)
                    .await
                    .map_err(|_| pause_host_store_unavailable())?;
                Ok(current.filter(|current| {
                    current.status == ChatHostStatus::Paused
                        && current.last_transition_claim.as_deref() == Some(claim)
                        && current.checkpoint.as_ref() == Some(checkpoint)
                        && current.paused_at.as_deref() == Some(now)
                }))
            }
        }
    }

    async fn finish_chat_pause(
        &self,
        hosts: &Arc<dyn ChatHostStore>,
        host_id: &ChatHostId,
        claim: &str,
        checkpoint: &ChatCheckpointRef,
        now: &str,
    ) -> Result<(), CoreError> {
        loop {
            let Some(current) = hosts
                .get(host_id)
                .await
                .map_err(|_| pause_host_store_unavailable())?
            else {
                return Err(CoreError::RuntimeUnavailable(
                    "the paused chat host disappeared after runtime teardown".into(),
                ));
            };
            if current.status == ChatHostStatus::Paused
                && current.runtime_id.is_none()
                && current.last_transition_claim.is_none()
                && current.checkpoint.as_ref() == Some(checkpoint)
            {
                return Ok(());
            }
            if current.status != ChatHostStatus::Paused
                || current.last_transition_claim.as_deref() != Some(claim)
                || current.checkpoint.as_ref() != Some(checkpoint)
            {
                return Err(CoreError::RuntimeUnavailable(
                    "the chat pause claim changed after runtime teardown".into(),
                ));
            }
            let mut finished = current.clone();
            finished.runtime_id = None;
            finished.last_transition_claim = None;
            finished.transition_started_at = None;
            finished.last_operator_note = None;
            finished.updated_at = now.to_string();
            match hosts
                .update_if_revision(current.revision, &finished)
                .await
                .map_err(|_| pause_host_store_unavailable())?
            {
                Some(_) => return Ok(()),
                None => continue,
            }
        }
    }

    async fn settle_chat_pause_claim(
        &self,
        hosts: &Arc<dyn ChatHostStore>,
        host_id: &ChatHostId,
        claim: &str,
        now: &str,
        restore_active: bool,
        refusal: Option<&'static str>,
    ) -> Result<(), CoreError> {
        loop {
            let Some(current) = hosts
                .get(host_id)
                .await
                .map_err(|_| pause_host_store_unavailable())?
            else {
                return Ok(());
            };
            if current.last_transition_claim.as_deref() != Some(claim) {
                return Ok(());
            }
            let mut replacement = current.clone();
            replacement.status = if restore_active {
                ChatHostStatus::Active
            } else {
                ChatHostStatus::Pausing
            };
            if restore_active {
                replacement.paused_at = None;
            }
            replacement.last_transition_claim = None;
            replacement.transition_started_at = None;
            if let Some(detail) = refusal {
                replacement.last_operator_note = Some(ChatHostOperatorNote {
                    code: "pause_refused".into(),
                    detail: detail.into(),
                    observed_at: now.into(),
                });
            }
            replacement.updated_at = now.into();
            if hosts
                .update_if_revision(current.revision, &replacement)
                .await
                .map_err(|_| pause_host_store_unavailable())?
                .is_some()
            {
                return Ok(());
            }
        }
    }
}

fn parse_pause_time(now: &str) -> Result<u64, CoreError> {
    crate::seconds_from_rfc3339(now).ok_or_else(|| {
        CoreError::InvalidRequest("chat inactivity time is not a CMA timestamp".into())
    })
}

fn pause_claimable(host: &ChatHostRecord, now_seconds: u64) -> bool {
    host.status == ChatHostStatus::Active
        && host.last_transition_claim.is_none()
        && host.runtime_id.is_some()
        && host.threads.iter().any(|thread| !thread.removed)
        && host.last_operator_note.as_ref().is_none_or(|note| {
            note.code != "pause_refused"
                || crate::seconds_from_rfc3339(&note.observed_at).is_some_and(|observed| {
                    now_seconds >= observed.saturating_add(CHAT_PAUSE_REFUSAL_BACKOFF_SECONDS)
                })
        })
}

fn next_checkpoint_version(host: &ChatHostRecord) -> Option<u64> {
    host.checkpoint_sequence
        .max(
            host.checkpoint
                .as_ref()
                .map_or(0, |checkpoint| checkpoint.checkpoint_version),
        )
        .checked_add(1)
}

fn chat_pause_notice(seconds: u32) -> String {
    let duration = if seconds.is_multiple_of(60) {
        format!("{}min", seconds / 60)
    } else {
        format!("{seconds}s")
    };
    format!("Your current chat session has been idle for {duration} and has been auto-paused.")
}

fn pause_host_store_unavailable() -> CoreError {
    CoreError::ChatUnavailable(crate::ChatUnavailableCause::HostStoreUnavailable)
}

fn pause_session_store_error(error: StoreError) -> CoreError {
    CoreError::RuntimeUnavailable(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_notice_formats_whole_minutes_and_exact_seconds() {
        assert_eq!(
            chat_pause_notice(900),
            "Your current chat session has been idle for 15min and has been auto-paused."
        );
        assert_eq!(
            chat_pause_notice(3_600),
            "Your current chat session has been idle for 60min and has been auto-paused."
        );
        assert_eq!(
            chat_pause_notice(901),
            "Your current chat session has been idle for 901s and has been auto-paused."
        );
    }
}
