//! Durable ownership and lifecycle state for one subject's shared chat runtime.

use std::collections::HashSet;
use std::num::NonZeroU64;
use std::sync::Arc;

use layer3_adapter::{
    AdapterError, RuntimeEnvironment, RuntimeHandle, RuntimeIdentity, RuntimeMode, RuntimeOptions,
    RuntimePermission, RuntimeSpec, TermSize, WorkspaceSourceRequest,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::{Page, PageRequest, MAX_PAGE_SIZE};
use crate::{
    Caller, Capability, ChatCheckpointRef, ChatSettingsSnapshot, CoreError,
    CredentialDeliveryTarget, KernelId, SessionId, SubjectId,
};

const CHAT_HOST_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x1fb7_66ee_28df_5e10_a833_2697_9a23_f36d);

/// Upper bounds keep the owner aggregate comfortably below MongoDB's document
/// limit even when every retained prompt is at its maximum size.
pub const MAX_CHAT_PROMPT_ADMISSIONS: usize = 64;
pub const MAX_CHAT_PROMPT_BYTES: usize = 96 * 1024;

/// Stable identity of the one chat host assigned to a subject.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChatHostId(pub String);

impl ChatHostId {
    /// Derive the same opaque host identity on every control-plane replica.
    pub fn for_owner(owner: &SubjectId) -> Self {
        let id = uuid::Uuid::new_v5(&CHAT_HOST_NAMESPACE, owner.as_str().as_bytes());
        Self(format!("chho_{}", id.simple()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Credential custody remains on the existing session-scoped port while
    /// receiving an identity that no user session can mint.
    pub(crate) fn credential_session_id(&self, generation: u64) -> SessionId {
        let base = self.0.trim_start_matches("chho_");
        if generation == 0 {
            SessionId(format!("sesn_chat_{base}"))
        } else {
            SessionId(format!("sesn_chat_{base}_cg{generation}"))
        }
    }

    /// Recover a host and credential incarnation from its synthetic session id.
    pub(crate) fn from_credential_session_id(id: &SessionId) -> Option<(Self, u64)> {
        let encoded = id.as_str().strip_prefix("sesn_chat_")?;
        let (base, generation) = match encoded.split_once("_cg") {
            Some((base, encoded_generation)) => {
                let generation = encoded_generation.parse::<u64>().ok()?;
                if generation == 0 || generation.to_string() != encoded_generation {
                    return None;
                }
                (base, generation)
            }
            None => (encoded, 0),
        };
        if base.len() != 32
            || !base
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return None;
        }
        Some((Self(format!("chho_{base}")), generation))
    }
}

impl std::fmt::Display for ChatHostId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Lifecycle of one shared runtime and its collection of threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatHostStatus {
    Provisioning,
    Active,
    Pausing,
    Paused,
    Reviving,
    Retired,
}

/// Why a replacement shared runtime is being created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatReviveCause {
    OwnerMessage,
    ImageDrift,
    RuntimeLost,
}

/// Durable progress through a restart-resumable host revival.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRevivePhase {
    CheckpointVerification,
    RuntimeProvisioning,
    ConversationRestore,
    WorkspaceSetup,
    ThreadRestore,
    PromptDelivery,
    RecoveryRequired,
}

/// Conservative delivery state for one prompt accepted while a host is down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatPromptDeliveryState {
    Queued,
    DispatchFenced,
    Delivered,
    RecoveryRequired,
}

/// One owner prompt durably admitted before replacement provisioning begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatPromptAdmission {
    pub id: String,
    pub sequence: u64,
    pub session_id: SessionId,
    pub payload: String,
    pub payload_sha256: String,
    /// Linear-turn authority retained for interfaces that prohibit forks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) linear_prompt: Option<crate::conversation::LinearPromptContext>,
    pub admitted_at: String,
    pub state: ChatPromptDeliveryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_fenced_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_started: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_phase: Option<ChatRevivePhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_provisioning_phase: Option<ChatProvisioningPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_command_index: Option<usize>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub failure_retryable: bool,
    /// Whether prompt delivery proved that another linear turn won first.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) linear_conflict: bool,
}

impl ChatPromptAdmission {
    pub(crate) fn new(session_id: SessionId, payload: String, sequence: u64, now: String) -> Self {
        let payload_sha256 = hex_sha256(payload.as_bytes());
        Self {
            id: format!("chpr_{}", uuid::Uuid::new_v4().simple()),
            sequence,
            session_id,
            payload,
            payload_sha256,
            linear_prompt: None,
            admitted_at: now,
            state: ChatPromptDeliveryState::Queued,
            dispatch_fenced_at: None,
            turn_started: None,
            failure_phase: None,
            failure_provisioning_phase: None,
            failure_command_index: None,
            failure_retryable: false,
            linear_conflict: false,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if !self.id.starts_with("chpr_") || self.id.len() != 37 {
            return Err("chat prompt admission identity is malformed".into());
        }
        if self.sequence == 0 {
            return Err("chat prompt admission sequence must be positive".into());
        }
        if self.payload.len() > MAX_CHAT_PROMPT_BYTES {
            return Err("chat prompt admission payload is outside its byte bound".into());
        }
        if self.payload_sha256 != hex_sha256(self.payload.as_bytes()) {
            return Err("chat prompt admission payload digest does not match".into());
        }
        if let Some(context) = self.linear_prompt.as_ref() {
            context.validate()?;
        }
        if crate::seconds_from_rfc3339(&self.admitted_at).is_none() {
            return Err("chat prompt admission timestamp is malformed".into());
        }
        if self
            .dispatch_fenced_at
            .as_deref()
            .is_some_and(|value| crate::seconds_from_rfc3339(value).is_none())
        {
            return Err("chat prompt dispatch fence timestamp is malformed".into());
        }
        match self.state {
            ChatPromptDeliveryState::Queued if self.dispatch_fenced_at.is_some() => {
                return Err("queued chat prompt retains a dispatch fence".into());
            }
            ChatPromptDeliveryState::DispatchFenced if self.dispatch_fenced_at.is_none() => {
                return Err("fenced chat prompt has no dispatch timestamp".into());
            }
            ChatPromptDeliveryState::Delivered if self.turn_started.is_none() => {
                return Err("delivered chat prompt has no durable turn boundary".into());
            }
            ChatPromptDeliveryState::Delivered
                if self.failure_phase.is_some()
                    || self.failure_provisioning_phase.is_some()
                    || self.failure_command_index.is_some()
                    || self.failure_retryable
                    || self.linear_conflict =>
            {
                return Err("delivered chat prompt retains a failure phase".into());
            }
            ChatPromptDeliveryState::RecoveryRequired if self.failure_phase.is_none() => {
                return Err("failed chat prompt has no typed recovery phase".into());
            }
            ChatPromptDeliveryState::RecoveryRequired if self.turn_started.is_some() => {
                return Err("failed chat prompt retains a delivered turn boundary".into());
            }
            ChatPromptDeliveryState::RecoveryRequired
                if self.linear_conflict
                    && (self.failure_phase != Some(ChatRevivePhase::PromptDelivery)
                        || self.failure_provisioning_phase.is_some()
                        || self.failure_command_index.is_some()
                        || self.failure_retryable) =>
            {
                return Err("linear conflict carries an inconsistent recovery outcome".into());
            }
            ChatPromptDeliveryState::Queued | ChatPromptDeliveryState::DispatchFenced
                if self.turn_started.is_some()
                    || self.failure_phase.is_some()
                    || self.failure_provisioning_phase.is_some()
                    || self.failure_command_index.is_some()
                    || self.failure_retryable
                    || self.linear_conflict =>
            {
                return Err("undelivered chat prompt carries a terminal outcome".into());
            }
            _ => {}
        }
        if self.failure_command_index.is_some()
            && self.failure_provisioning_phase != Some(ChatProvisioningPhase::WorkspaceSetup)
        {
            return Err("chat prompt setup failure index has no setup phase".into());
        }
        Ok(())
    }
}

/// A restore refusal scoped to one thread rather than its healthy siblings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatThreadRestoreFailure {
    pub session_id: SessionId,
    pub phase: ChatRevivePhase,
    pub retryable: bool,
    pub occurred_at: String,
}

/// One session/worktree allocation retained for the lifetime of its host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatThreadSlot {
    pub session_id: SessionId,
    pub worktree_name: String,
    pub worktree_path: String,
    pub created_at: String,
    pub removed: bool,
}

/// Bounded phase vocabulary for an operator-visible host failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatProvisioningPhase {
    Runtime,
    WorkspaceSource,
    WorkspaceSetup,
    Credential,
    Cleanup,
    RuntimeLost,
}

/// Why the chat surface cannot currently provide a usable host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatUnavailableCause {
    NotConfigured,
    Disabled,
    HostStoreUnavailable,
    Provisioning,
    RevivePending,
    Retired,
    CapabilityUnsupported(String),
    RestoreFailed {
        phase: ChatRevivePhase,
        retryable: bool,
    },
    ProvisioningFailed {
        phase: ChatProvisioningPhase,
        command_index: Option<usize>,
        retryable: bool,
    },
}

impl ChatUnavailableCause {
    /// Transient causes map to service unavailability; the rest are policy or
    /// composition refusals and map to an unprocessable request.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::NotConfigured
                | Self::HostStoreUnavailable
                | Self::Provisioning
                | Self::RevivePending
                | Self::RestoreFailed {
                    retryable: true,
                    ..
                }
                | Self::ProvisioningFailed {
                    retryable: true,
                    ..
                }
        )
    }
}

impl std::fmt::Display for ChatUnavailableCause {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => formatter.write_str("chat is not configured"),
            Self::Disabled => formatter.write_str("chat is disabled"),
            Self::HostStoreUnavailable => formatter.write_str("chat host storage is unavailable"),
            Self::Provisioning => formatter.write_str("the chat host is still provisioning"),
            Self::RevivePending => formatter.write_str("chat host revival is pending"),
            Self::Retired => formatter.write_str("the chat host is retired"),
            Self::CapabilityUnsupported(capability) => {
                write!(formatter, "the chat runtime does not support {capability}")
            }
            Self::RestoreFailed { phase, .. } => {
                write!(formatter, "chat restoration failed during {phase:?}")
            }
            Self::ProvisioningFailed {
                phase,
                command_index,
                ..
            } => {
                write!(formatter, "chat host provisioning failed during {phase:?}")?;
                if let Some(index) = command_index {
                    write!(formatter, " at command {index}")?;
                }
                Ok(())
            }
        }
    }
}

/// Sanitized failure retained on a retryable host record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatHostFailure {
    pub phase: ChatProvisioningPhase,
    pub command_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_excerpt: Option<String>,
    pub retryable: bool,
    pub cleanup_pending: bool,
    pub occurred_at: String,
}

/// Exact immutable source authority installed in the host's current runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatWorkspaceAuthority {
    pub workspace_epoch: u64,
    pub source_revision: String,
    pub baseline_manifest_digest: String,
}

impl ChatWorkspaceAuthority {
    fn from_receipt(receipt: layer3_adapter::WorkspaceSourceReceipt) -> Result<Self, String> {
        let authority = Self {
            workspace_epoch: receipt.workspace_epoch,
            source_revision: receipt.source_revision,
            baseline_manifest_digest: receipt.baseline_manifest_digest,
        };
        authority.validate()?;
        Ok(authority)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.workspace_epoch == 0 {
            return Err("chat workspace epoch must be positive".into());
        }
        if self.source_revision.len() != 40
            || !self
                .source_revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("chat workspace source revision must be a Git SHA-1".into());
        }
        if self.baseline_manifest_digest.len() != 64
            || !self
                .baseline_manifest_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("chat workspace baseline must be a SHA-256 digest".into());
        }
        Ok(())
    }
}

/// Sanitized lifecycle note retained for operator inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatHostOperatorNote {
    pub code: String,
    pub detail: String,
    pub observed_at: String,
}

/// The complete durable aggregate for one subject's chat runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatHostRecord {
    pub id: ChatHostId,
    pub owner: SubjectId,
    pub status: ChatHostStatus,
    pub pinned_settings: ChatSettingsSnapshot,
    /// Fully rendered deployment briefing pinned at host creation.
    pub platform_briefing: String,
    /// Deployment-selected adapter identity pinned with the host.
    pub kernel: KernelId,
    pub runtime_id: Option<String>,
    pub runtime_version: Option<String>,
    /// Provider fencing generation. Independent from image-rotation accounting
    /// and from the workspace epoch sealed inside a checkpoint archive.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub runtime_generation: u64,
    /// Incarnation of credential custody for this deterministic owner host.
    /// Zero preserves the synthetic id used before this field was introduced.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub credential_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_authority: Option<ChatWorkspaceAuthority>,
    pub threads: Vec<ChatThreadSlot>,
    pub threads_created: u32,
    pub checkpoint: Option<ChatCheckpointRef>,
    /// Monotonic workspace capture version reserved by every checkpoint attempt.
    /// Failed attempts consume their version because Layer 4 capture identities
    /// are immutable even when publication never reaches this record.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub checkpoint_sequence: u64,
    pub paused_at: Option<String>,
    pub revived_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotated_at: Option<String>,
    pub retired_at: Option<String>,
    pub last_transition_claim: Option<String>,
    /// Start of the current claim, retained for diagnostics and later leases.
    pub transition_started_at: Option<String>,
    pub last_failure: Option<ChatHostFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_operator_note: Option<ChatHostOperatorNote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revive_cause: Option<ChatReviveCause>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revive_phase: Option<ChatRevivePhase>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revive_failed_threads: Vec<ChatThreadRestoreFailure>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub prompt_admission_sequence: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompt_admissions: Vec<ChatPromptAdmission>,
    pub rotations: u32,
    pub created_at: String,
    pub updated_at: String,
    /// Store-owned optimistic revision. Zero exists only before first insert.
    pub revision: u64,
}

impl ChatHostRecord {
    pub(crate) fn new(
        owner: SubjectId,
        pinned_settings: ChatSettingsSnapshot,
        platform_briefing: String,
        kernel: KernelId,
        now: String,
        claim: String,
    ) -> Self {
        Self {
            id: ChatHostId::for_owner(&owner),
            owner,
            status: ChatHostStatus::Provisioning,
            pinned_settings,
            platform_briefing,
            kernel,
            runtime_id: None,
            runtime_version: None,
            runtime_generation: 1,
            credential_generation: 0,
            workspace_authority: None,
            threads: Vec::new(),
            threads_created: 0,
            checkpoint: None,
            checkpoint_sequence: 0,
            paused_at: None,
            revived_at: None,
            rotated_at: None,
            retired_at: None,
            last_transition_claim: Some(claim),
            transition_started_at: Some(now.clone()),
            last_failure: None,
            last_operator_note: None,
            revive_cause: None,
            revive_phase: None,
            revive_failed_threads: Vec::new(),
            prompt_admission_sequence: 0,
            prompt_admissions: Vec::new(),
            rotations: 0,
            created_at: now.clone(),
            updated_at: now,
            revision: 0,
        }
    }

    pub(crate) fn credential_session_id(&self) -> SessionId {
        self.id.credential_session_id(self.credential_generation)
    }

    /// Reject corrupt durable state before it can authorize provider work.
    pub fn validate(&self) -> Result<(), String> {
        if self.id != ChatHostId::for_owner(&self.owner) {
            return Err("chat host identity does not match its owner".into());
        }
        if self.kernel.is_unset() {
            return Err("chat host kernel is empty".into());
        }
        if self.last_transition_claim.is_some() != self.transition_started_at.is_some() {
            return Err("chat host transition claim is incomplete".into());
        }
        if let Some(authority) = self.workspace_authority.as_ref() {
            authority.validate()?;
        }
        if let Some(failure) = self.last_failure.as_ref() {
            if crate::seconds_from_rfc3339(&failure.occurred_at).is_none()
                || failure.output_excerpt.as_ref().is_some_and(|excerpt| {
                    excerpt.len() > layer3_adapter::MAX_WORKSPACE_SETUP_OUTPUT_BYTES
                })
                || (failure.command_index.is_some()
                    && failure.phase != ChatProvisioningPhase::WorkspaceSetup)
                || (failure.output_excerpt.is_some()
                    && (failure.phase != ChatProvisioningPhase::WorkspaceSetup
                        || failure.command_index.is_none()))
            {
                return Err("chat host failure is malformed".into());
            }
        }
        if self.runtime_generation != 0 && self.runtime_generation <= u64::from(self.rotations) {
            return Err("chat runtime generation does not cover its rotations".into());
        }
        if let Some(checkpoint) = self.checkpoint.as_ref() {
            checkpoint.validate()?;
            if self.checkpoint_sequence != 0
                && self.checkpoint_sequence < checkpoint.checkpoint_version
            {
                return Err("chat checkpoint sequence is behind its published version".into());
            }
        }
        if let Some(note) = self.last_operator_note.as_ref() {
            if !matches!(
                note.code.as_str(),
                "checkpoint_refused" | "pause_refused" | "revive_refused" | "recovery_required"
            ) || note.detail.trim().is_empty()
                || note.detail.len() > 256
                || crate::seconds_from_rfc3339(&note.observed_at).is_none()
            {
                return Err("chat host operator note is malformed".into());
            }
        }
        if self.threads.len() > usize::from(self.pinned_settings.settings.host.max_threads) {
            return Err("chat host contains more threads than its pinned limit".into());
        }
        if self.threads_created < u32::try_from(self.threads.len()).unwrap_or(u32::MAX) {
            return Err("chat host thread counter precedes its retained slots".into());
        }
        let mut sessions = HashSet::with_capacity(self.threads.len());
        let mut worktrees = HashSet::with_capacity(self.threads.len());
        for thread in &self.threads {
            if !sessions.insert(thread.session_id.as_str()) {
                return Err("chat host repeats a session slot".into());
            }
            if !worktrees.insert(thread.worktree_name.as_str()) {
                return Err("chat host repeats a worktree slot".into());
            }
        }
        if self.prompt_admissions.len() > MAX_CHAT_PROMPT_ADMISSIONS {
            return Err("chat host retains too many prompt admissions".into());
        }
        let retained_sessions = self
            .threads
            .iter()
            .filter(|thread| !thread.removed)
            .map(|thread| thread.session_id.as_str())
            .collect::<HashSet<_>>();
        let mut admission_ids = HashSet::with_capacity(self.prompt_admissions.len());
        let mut previous_sequence = 0;
        for admission in &self.prompt_admissions {
            admission.validate()?;
            if !retained_sessions.contains(admission.session_id.as_str()) {
                return Err("chat prompt admission names a non-retained thread".into());
            }
            if !admission_ids.insert(admission.id.as_str()) {
                return Err("chat host repeats a prompt admission identity".into());
            }
            if admission.sequence <= previous_sequence
                || admission.sequence > self.prompt_admission_sequence
            {
                return Err("chat prompt admissions are not in durable sequence order".into());
            }
            previous_sequence = admission.sequence;
        }
        if self.prompt_admissions.is_empty() && self.prompt_admission_sequence == 0 {
            // Initial state.
        } else if self.prompt_admission_sequence == 0 {
            return Err("chat prompt admission counter is behind retained admissions".into());
        }
        let mut failed_sessions = HashSet::with_capacity(self.revive_failed_threads.len());
        for failure in &self.revive_failed_threads {
            if !retained_sessions.contains(failure.session_id.as_str())
                || !failed_sessions.insert(failure.session_id.as_str())
                || crate::seconds_from_rfc3339(&failure.occurred_at).is_none()
            {
                return Err("chat thread restore failure is malformed".into());
            }
        }
        Ok(())
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod prompt_admission_tests {
    use super::*;

    #[test]
    fn contradictory_terminal_prompt_outcomes_are_rejected() {
        let mut failed = ChatPromptAdmission::new(
            SessionId::generate(),
            "retained payload".into(),
            1,
            "2026-09-01T00:00:00Z".into(),
        );
        failed.state = ChatPromptDeliveryState::RecoveryRequired;
        failed.failure_phase = Some(ChatRevivePhase::PromptDelivery);
        failed.turn_started = Some(7);
        assert_eq!(
            failed.validate().unwrap_err(),
            "failed chat prompt retains a delivered turn boundary"
        );

        let mut delivered = failed;
        delivered.state = ChatPromptDeliveryState::Delivered;
        delivered.turn_started = Some(7);
        assert_eq!(
            delivered.validate().unwrap_err(),
            "delivered chat prompt retains a failure phase"
        );
    }
}

/// Filters attached to one bounded host listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatHostListQuery {
    pub page: PageRequest,
    pub statuses: Vec<ChatHostStatus>,
}

impl ChatHostListQuery {
    pub fn new(page: PageRequest, statuses: Vec<ChatHostStatus>) -> Self {
        let mut unique = Vec::new();
        for status in statuses.into_iter().take(MAX_PAGE_SIZE as usize) {
            if !unique.contains(&status) {
                unique.push(status);
            }
        }
        Self {
            page,
            statuses: unique,
        }
    }
}

impl Default for ChatHostListQuery {
    fn default() -> Self {
        Self::new(PageRequest::default(), Vec::new())
    }
}

fn chat_unavailable(cause: ChatUnavailableCause) -> CoreError {
    CoreError::ChatUnavailable(cause)
}

fn host_store_unavailable() -> CoreError {
    chat_unavailable(ChatUnavailableCause::HostStoreUnavailable)
}

pub(crate) fn generation_for(host: &ChatHostRecord) -> NonZeroU64 {
    NonZeroU64::new(if host.runtime_generation == 0 {
        u64::from(host.rotations).saturating_add(1)
    } else {
        host.runtime_generation
    })
    .expect("a chat host generation is always non-zero")
}

fn hex_sha256(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn failure_cause(failure: &ChatHostFailure) -> ChatUnavailableCause {
    ChatUnavailableCause::ProvisioningFailed {
        phase: failure.phase,
        command_index: failure.command_index,
        retryable: failure.retryable,
    }
}

impl crate::service::SessionService {
    /// Whether the deployment currently exposes the owner-facing chat surface.
    ///
    /// Capability discovery fails closed: an unreadable settings revision
    /// cannot safely be replaced by shipped defaults because that could expose
    /// a surface the administrator disabled.
    pub async fn chat_surface_enabled(&self) -> bool {
        match self.settings.chat_snapshot().await {
            Ok(Some(snapshot)) => snapshot.settings.enabled,
            Ok(None) => false,
            Err(error) => {
                tracing::warn!(%error, "chat capability discovery could not read platform settings");
                false
            }
        }
    }

    /// Read one bounded, newest-first page of deployment-managed chat hosts.
    pub async fn list_chat_hosts(
        &self,
        caller: &Caller,
        query: &ChatHostListQuery,
    ) -> Result<Page<ChatHostRecord>, CoreError> {
        caller.authorization.require(Capability::ReadAllSessions)?;
        self.chat_hosts
            .as_ref()
            .ok_or_else(host_store_unavailable)?
            .list(query)
            .await
            .map_err(|_| host_store_unavailable())
    }

    async fn fresh_chat_host_candidate(
        &self,
        owner: &SubjectId,
    ) -> Result<ChatHostRecord, CoreError> {
        let policy = self
            .settings
            .chat_host_policy()
            .await?
            .ok_or_else(|| chat_unavailable(ChatUnavailableCause::NotConfigured))?;
        if !policy.snapshot.settings.enabled {
            return Err(chat_unavailable(ChatUnavailableCause::Disabled));
        }

        let kernel = self.environments.default_agent().kernel.clone();
        let adapter = self.adapters.get(&kernel).ok_or_else(|| {
            chat_unavailable(ChatUnavailableCause::CapabilityUnsupported(
                "configured chat kernel".into(),
            ))
        })?;
        let capabilities = adapter.capabilities();
        if !capabilities.conversation {
            return Err(chat_unavailable(
                ChatUnavailableCause::CapabilityUnsupported("conversation".into()),
            ));
        }
        if !capabilities.snapshot {
            return Err(chat_unavailable(
                ChatUnavailableCause::CapabilityUnsupported("snapshot".into()),
            ));
        }

        let now = crate::clock::now_rfc3339();
        let service_names = self
            .environments
            .baseline()
            .services()
            .iter()
            .map(|service| service.0.clone())
            .collect::<Vec<_>>();
        Ok(ChatHostRecord::new(
            owner.clone(),
            policy.snapshot,
            policy.briefing.render(&service_names),
            kernel,
            now,
            format!("chcl_{}", uuid::Uuid::new_v4().simple()),
        ))
    }

    /// Acquire foreground ownership while periodically invalidating only an
    /// opportunistic checkpoint that currently holds it.
    pub(super) async fn foreground_chat_host_guard(
        &self,
        owner: &SubjectId,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        loop {
            if let Some(guard) = self.chat_host_guards.try_lock(owner).await {
                return guard;
            }
            self.preempt_periodic_chat_checkpoint(owner).await;
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    async fn preempt_periodic_chat_checkpoint(&self, owner: &SubjectId) {
        let Some(hosts) = self.chat_hosts.as_ref() else {
            return;
        };
        let Ok(Some(host)) = hosts.get_for_owner(owner).await else {
            return;
        };
        if host.status != ChatHostStatus::Active
            || !host
                .last_transition_claim
                .as_deref()
                .is_some_and(|claim| claim.starts_with("chcp_"))
        {
            return;
        }
        let (Some(runtime_id), Some(adapter)) =
            (host.runtime_id.as_ref(), self.adapters.get(&host.kernel))
        else {
            return;
        };
        match adapter
            .preempt_workspace_checkpoint(&RuntimeHandle(runtime_id.clone()))
            .await
        {
            Ok(true) => tracing::info!(
                chat_host = %host.id,
                "foreground Chat creation preempted a periodic workspace checkpoint"
            ),
            Ok(false) | Err(AdapterError::Unsupported(_)) => {}
            Err(error) => tracing::warn!(
                chat_host = %host.id,
                %error,
                "foreground Chat creation could not preempt a periodic workspace checkpoint"
            ),
        }
    }

    /// Return this owner's one usable shared chat host, provisioning it once
    /// when no durable host exists.
    pub(crate) async fn ensure_chat_host(
        self: &Arc<Self>,
        owner: &SubjectId,
        caller_credential: Option<&crate::CallerCredential>,
    ) -> Result<ChatHostRecord, CoreError> {
        let store = self.chat_hosts.clone().ok_or_else(host_store_unavailable)?;
        let _owner_guard = self.foreground_chat_host_guard(owner).await;

        let existing = store
            .get_for_owner(owner)
            .await
            .map_err(|_| host_store_unavailable())?;
        let host = match existing {
            Some(host) if host.status == ChatHostStatus::Active => return Ok(host),
            Some(host) if host.status == ChatHostStatus::Retired => {
                let next_generation =
                    generation_for(&host).get().checked_add(1).ok_or_else(|| {
                        CoreError::InvalidRequest("chat runtime generation exhausted".into())
                    })?;
                let next_credential_generation =
                    host.credential_generation.checked_add(1).ok_or_else(|| {
                        CoreError::InvalidRequest("chat credential generation exhausted".into())
                    })?;
                let checkpoint_sequence = host.checkpoint_sequence.max(
                    host.checkpoint
                        .as_ref()
                        .map_or(0, |checkpoint| checkpoint.checkpoint_version),
                );
                let mut candidate = self.fresh_chat_host_candidate(owner).await?;
                candidate.runtime_generation = next_generation;
                candidate.credential_generation = next_credential_generation;
                candidate.checkpoint_sequence = checkpoint_sequence;
                candidate.revision = host.revision;
                store
                    .update_if_revision(host.revision, &candidate)
                    .await
                    .map_err(|_| host_store_unavailable())?
                    .ok_or_else(|| chat_unavailable(ChatUnavailableCause::Provisioning))?
            }
            Some(mut host)
                if host.status == ChatHostStatus::Provisioning
                    && host.credential_generation == 0
                    && host.last_transition_claim.is_none()
                    && host.runtime_id.is_none()
                    && host.threads.iter().all(|thread| thread.removed)
                    && host.last_failure.as_ref().is_some_and(|failure| {
                        failure.phase == ChatProvisioningPhase::Credential
                            && !failure.retryable
                            && !failure.cleanup_pending
                    }) =>
            {
                host.credential_generation = 1;
                host.last_failure = None;
                host.last_transition_claim =
                    Some(format!("chcl_{}", uuid::Uuid::new_v4().simple()));
                host.transition_started_at = Some(crate::clock::now_rfc3339());
                host.updated_at = crate::clock::now_rfc3339();
                let expected = host.revision;
                store
                    .update_if_revision(expected, &host)
                    .await
                    .map_err(|_| host_store_unavailable())?
                    .ok_or_else(|| chat_unavailable(ChatUnavailableCause::Provisioning))?
            }
            Some(host)
                if matches!(
                    host.status,
                    ChatHostStatus::Paused | ChatHostStatus::Pausing | ChatHostStatus::Reviving
                ) =>
            {
                return Err(chat_unavailable(ChatUnavailableCause::RevivePending));
            }
            Some(mut host)
                if host.status == ChatHostStatus::Provisioning
                    && host.last_transition_claim.is_none()
                    && host.runtime_id.is_none()
                    && host
                        .last_failure
                        .as_ref()
                        .is_some_and(|failure| failure.retryable && !failure.cleanup_pending) =>
            {
                let claim = format!("chcl_{}", uuid::Uuid::new_v4().simple());
                host.last_transition_claim = Some(claim);
                host.transition_started_at = Some(crate::clock::now_rfc3339());
                host.updated_at = crate::clock::now_rfc3339();
                let expected = host.revision;
                store
                    .update_if_revision(expected, &host)
                    .await
                    .map_err(|_| host_store_unavailable())?
                    .ok_or_else(|| chat_unavailable(ChatUnavailableCause::Provisioning))?
            }
            Some(host) => {
                if let Some(failure) = host.last_failure.as_ref() {
                    return Err(chat_unavailable(failure_cause(failure)));
                }
                return Err(chat_unavailable(ChatUnavailableCause::Provisioning));
            }
            None => {
                let candidate = self.fresh_chat_host_candidate(owner).await?;
                let host = store
                    .create_if_absent(&candidate)
                    .await
                    .map_err(|_| host_store_unavailable())?;
                if host.status == ChatHostStatus::Active {
                    return Ok(host);
                }
                if host.last_transition_claim != candidate.last_transition_claim {
                    return Err(chat_unavailable(ChatUnavailableCause::Provisioning));
                }
                host
            }
        };

        if !self
            .chat_hosts_starting
            .write()
            .await
            .insert(host.id.clone())
        {
            return Err(chat_unavailable(ChatUnavailableCause::Provisioning));
        }
        let result = self
            .provision_claimed_chat_host(host.clone(), caller_credential)
            .await;
        self.chat_hosts_starting.write().await.remove(&host.id);
        result
    }

    pub(crate) async fn provision_claimed_chat_host(
        self: &Arc<Self>,
        mut host: ChatHostRecord,
        caller_credential: Option<&crate::CallerCredential>,
    ) -> Result<ChatHostRecord, CoreError> {
        let adapter = self.adapters.get(&host.kernel).cloned().ok_or_else(|| {
            chat_unavailable(ChatUnavailableCause::CapabilityUnsupported(
                "configured chat kernel".into(),
            ))
        })?;
        let capabilities = adapter.capabilities();
        if !capabilities.conversation || !capabilities.snapshot {
            let capability = if !capabilities.conversation {
                "conversation"
            } else {
                "snapshot"
            };
            return self
                .fail_chat_host(
                    &host,
                    None,
                    ChatProvisioningPhase::Runtime,
                    None,
                    false,
                    Some(capability),
                )
                .await;
        }

        let baseline = self.environments.baseline();
        let services = baseline.services().to_vec();
        let service_names = services
            .iter()
            .map(|service| service.0.clone())
            .collect::<Vec<_>>();
        let wanted_skills = baseline
            .skills()
            .iter()
            .map(|skill| skill.0.clone())
            .collect::<Vec<_>>();
        let skill_packs = match self
            .resolve_skill_packs_for_creation(
                caller_credential.map(AsRef::<str>::as_ref),
                &wanted_skills,
            )
            .await
        {
            Ok(packs) => packs,
            Err(_) => {
                return self
                    .fail_chat_host(
                        &host,
                        None,
                        ChatProvisioningPhase::Runtime,
                        None,
                        true,
                        None,
                    )
                    .await;
            }
        };

        let credential_session = host.credential_session_id();
        match self
            .credential_for(
                &credential_session,
                &host.owner,
                &services,
                caller_credential,
            )
            .await
        {
            Ok(Some(credential)) => drop(credential),
            Ok(None) => {
                return self
                    .fail_chat_host(
                        &host,
                        None,
                        ChatProvisioningPhase::Credential,
                        None,
                        false,
                        None,
                    )
                    .await;
            }
            Err(_) => {
                return self
                    .fail_chat_host(
                        &host,
                        None,
                        ChatProvisioningPhase::Credential,
                        None,
                        true,
                        None,
                    )
                    .await;
            }
        }

        let settings = host.pinned_settings.settings.clone();
        let generation = generation_for(&host);
        let reservation_token = host
            .last_transition_claim
            .clone()
            .ok_or_else(|| chat_unavailable(ChatUnavailableCause::Provisioning))?;
        let options = RuntimeOptions {
            mode: RuntimeMode::Conversation,
            model: settings.agent.model.clone(),
            reasoning_effort: Some(settings.agent.reasoning_effort.as_str().to_string()),
            permission: RuntimePermission::Unrestricted,
            own_credential: false,
            restricted_service_gateway: false,
            working_directory: None,
            conversation_slot: None,
            resume_conversation: None,
        };
        let environment = RuntimeEnvironment {
            variables: Default::default(),
            packages: baseline
                .packages()
                .iter()
                .map(crate::SoftwarePackage::requirement)
                .collect(),
            skills: wanted_skills,
            agent_notes: settings.agent.instructions.clone(),
        };
        let spec = RuntimeSpec {
            session_id: host.id.0.clone(),
            chat_host: Some(host.id.0.clone()),
            generation: Some(generation),
            reservation_token: Some(reservation_token),
            size: TermSize {
                cols: 120,
                rows: 40,
            },
            capacity: crate::provision_support::runtime_capacity(settings.host.size),
            timeout_seconds: None,
            subject_credential: None,
            options,
            restore_from: None,
            workspace: None,
            created_by: self.identity.clone(),
            skills: skill_packs
                .iter()
                .map(|pack| layer3_adapter::SkillPackage {
                    name: pack.name.clone(),
                    files: pack.files.clone(),
                })
                .collect(),
            platform_briefing: host.platform_briefing.clone(),
            environment,
            reaches: service_names,
            runtime_policy_required: false,
            sponsored_model_required: false,
            workspace_source_required: true,
            conversation_slots_required: Some(settings.host.max_threads),
        };
        let handle = match adapter.provision(&spec).await {
            Ok(handle) => handle,
            Err(error) => {
                let unsupported = matches!(error, AdapterError::Unsupported(_));
                return self
                    .fail_chat_host(
                        &host,
                        None,
                        ChatProvisioningPhase::Runtime,
                        None,
                        !unsupported,
                        unsupported.then_some("workspace source and conversation slots"),
                    )
                    .await;
            }
        };

        host.runtime_id = Some(handle.0.clone());
        host.updated_at = crate::clock::now_rfc3339();
        let store = self
            .chat_hosts
            .as_ref()
            .ok_or_else(host_store_unavailable)?;
        host = match store.update_if_revision(host.revision, &host).await {
            Ok(Some(host)) => host,
            Ok(None) => {
                self.cleanup_unpublished_chat_runtime(&host, &handle, &adapter)
                    .await;
                return Err(chat_unavailable(ChatUnavailableCause::Provisioning));
            }
            Err(_) => {
                self.cleanup_unpublished_chat_runtime(&host, &handle, &adapter)
                    .await;
                return Err(host_store_unavailable());
            }
        };

        self.finish_claimed_chat_host_runtime(host, handle, adapter)
            .await
    }

    /// Complete idempotent source and setup work for an already-recorded live
    /// runtime. Reconciliation uses the same path after adopting a provider
    /// object created before an earlier process published readiness.
    pub(crate) async fn finish_claimed_chat_host_runtime(
        &self,
        mut host: ChatHostRecord,
        handle: RuntimeHandle,
        adapter: Arc<dyn layer3_adapter::KernelAdapter>,
    ) -> Result<ChatHostRecord, CoreError> {
        let settings = host.pinned_settings.settings.clone();
        let source = WorkspaceSourceRequest {
            repository_url: settings.workspace.repository_url,
            reference: settings.workspace.branch,
            directory: host.pinned_settings.workspace_directory.clone(),
            workspace_epoch: generation_for(&host).get(),
        };
        let source_authority = match adapter.prepare_workspace_source(&handle, &source).await {
            Ok(receipt) => match ChatWorkspaceAuthority::from_receipt(receipt) {
                Ok(authority) if authority.workspace_epoch == source.workspace_epoch => authority,
                Ok(_) | Err(_) => {
                    return self
                        .fail_chat_host(
                            &host,
                            Some((&handle, &adapter)),
                            ChatProvisioningPhase::WorkspaceSource,
                            None,
                            false,
                            None,
                        )
                        .await;
                }
            },
            Err(error) => {
                let unsupported = matches!(error, AdapterError::Unsupported(_));
                return self
                    .fail_chat_host(
                        &host,
                        Some((&handle, &adapter)),
                        ChatProvisioningPhase::WorkspaceSource,
                        None,
                        !unsupported,
                        unsupported.then_some("workspace_source"),
                    )
                    .await;
            }
        };
        let setup = match adapter
            .run_workspace_setup(&handle, &settings.setup.commands)
            .await
        {
            Ok(setup) => setup,
            Err(error) => {
                let unsupported = matches!(error, AdapterError::Unsupported(_));
                return self
                    .fail_chat_host(
                        &host,
                        Some((&handle, &adapter)),
                        ChatProvisioningPhase::WorkspaceSetup,
                        None,
                        !unsupported,
                        unsupported.then_some("workspace_setup"),
                    )
                    .await;
            }
        };
        if setup.validate(settings.setup.commands.len()).is_err() {
            return self
                .fail_chat_host(
                    &host,
                    Some((&handle, &adapter)),
                    ChatProvisioningPhase::WorkspaceSetup,
                    None,
                    false,
                    None,
                )
                .await;
        }
        if let Some(failed) = setup.failed_step() {
            return self
                .fail_chat_host(
                    &host,
                    Some((&handle, &adapter)),
                    ChatProvisioningPhase::WorkspaceSetup,
                    Some(failed),
                    true,
                    None,
                )
                .await;
        }

        host.status = ChatHostStatus::Active;
        host.runtime_version = self.runtime_version.clone();
        host.workspace_authority = Some(source_authority);
        host.last_transition_claim = None;
        host.transition_started_at = None;
        host.last_failure = None;
        host.updated_at = crate::clock::now_rfc3339();
        let store = self
            .chat_hosts
            .as_ref()
            .ok_or_else(host_store_unavailable)?;
        match store
            .update_if_revision(host.revision, &host)
            .await
            .map_err(|_| host_store_unavailable())?
        {
            Some(host) => Ok(host),
            None => {
                let current = store
                    .get(&host.id)
                    .await
                    .map_err(|_| host_store_unavailable())?;
                if let Some(current) = current {
                    if current.status == ChatHostStatus::Active
                        && current.runtime_id.as_deref() == Some(handle.0.as_str())
                    {
                        return Ok(current);
                    }
                    if current.runtime_id.as_deref() == Some(handle.0.as_str()) {
                        return Err(chat_unavailable(ChatUnavailableCause::Provisioning));
                    }
                }
                self.cleanup_unpublished_chat_runtime(&host, &handle, &adapter)
                    .await;
                Err(chat_unavailable(ChatUnavailableCause::Provisioning))
            }
        }
    }

    async fn cleanup_unpublished_chat_runtime(
        &self,
        host: &ChatHostRecord,
        handle: &RuntimeHandle,
        adapter: &Arc<dyn layer3_adapter::KernelAdapter>,
    ) {
        let identity = RuntimeIdentity {
            handle: handle.clone(),
            session: host.id.0.clone(),
            generation: Some(generation_for(host)),
        };
        if let Err(error) = adapter.terminate_if_identity(&identity).await {
            tracing::warn!(
                chat_host = %host.id,
                runtime = %handle.0,
                %error,
                "an unpublished chat runtime could not be conditionally removed"
            );
        }
    }

    async fn fail_chat_host(
        &self,
        observed: &ChatHostRecord,
        runtime: Option<(&RuntimeHandle, &Arc<dyn layer3_adapter::KernelAdapter>)>,
        phase: ChatProvisioningPhase,
        setup_failure: Option<&layer3_adapter::WorkspaceSetupStep>,
        retryable: bool,
        unsupported_capability: Option<&str>,
    ) -> Result<ChatHostRecord, CoreError> {
        let cleanup_pending = if let Some((handle, adapter)) = runtime {
            let identity = RuntimeIdentity {
                handle: handle.clone(),
                session: observed.id.0.clone(),
                generation: Some(generation_for(observed)),
            };
            adapter.terminate_if_identity(&identity).await.is_err()
        } else {
            false
        };
        let failure = ChatHostFailure {
            phase,
            command_index: setup_failure.map(|step| step.command_index),
            output_excerpt: setup_failure.map(|step| step.output_excerpt.clone()),
            retryable,
            cleanup_pending,
            occurred_at: crate::clock::now_rfc3339(),
        };
        if let Some(store) = self.chat_hosts.as_ref() {
            if let Ok(Some(mut current)) = store.get(&observed.id).await {
                if current.last_transition_claim == observed.last_transition_claim
                    && (observed.runtime_id.is_none() || current.runtime_id == observed.runtime_id)
                {
                    if !cleanup_pending {
                        current.runtime_id = None;
                    }
                    current.status = ChatHostStatus::Provisioning;
                    current.last_failure = Some(failure.clone());
                    current.last_transition_claim = None;
                    current.transition_started_at = None;
                    current.updated_at = crate::clock::now_rfc3339();
                    let _ = store.update_if_revision(current.revision, &current).await;
                }
            }
        }
        if let Some(capability) = unsupported_capability {
            return Err(chat_unavailable(
                ChatUnavailableCause::CapabilityUnsupported(capability.into()),
            ));
        }
        Err(chat_unavailable(failure_cause(&failure)))
    }

    /// Install the host-scoped delegated credential after a first conversation
    /// opens the runtime control lane.
    pub(crate) async fn install_chat_host_credential(
        &self,
        host_id: &ChatHostId,
        caller_credential: Option<&crate::CallerCredential>,
    ) -> Result<(), CoreError> {
        let store = self
            .chat_hosts
            .as_ref()
            .ok_or_else(host_store_unavailable)?;
        let host = store
            .get(host_id)
            .await
            .map_err(|_| host_store_unavailable())?
            .ok_or_else(|| chat_unavailable(ChatUnavailableCause::Provisioning))?;
        if !matches!(
            host.status,
            ChatHostStatus::Active | ChatHostStatus::Reviving
        ) {
            return Err(chat_unavailable(ChatUnavailableCause::Provisioning));
        }
        let runtime_id = host
            .runtime_id
            .clone()
            .ok_or_else(|| chat_unavailable(ChatUnavailableCause::Provisioning))?;
        let adapter = self.adapters.get(&host.kernel).cloned().ok_or_else(|| {
            chat_unavailable(ChatUnavailableCause::CapabilityUnsupported(
                "configured chat kernel".into(),
            ))
        })?;
        let source = self.credentials.as_ref().ok_or_else(|| {
            chat_unavailable(ChatUnavailableCause::ProvisioningFailed {
                phase: ChatProvisioningPhase::Credential,
                command_index: None,
                retryable: false,
            })
        })?;
        let services = self.environments.baseline().services().to_vec();
        let target = CredentialDeliveryTarget {
            runtime_generation: generation_for(&host).get(),
            runtime_id: runtime_id.clone(),
        };
        let credential_session = host.credential_session_id();
        let credential = source
            .prepare_credential_delivery(
                &credential_session,
                &host.owner,
                &services,
                caller_credential,
                &target,
            )
            .await
            .map_err(|_| {
                chat_unavailable(ChatUnavailableCause::ProvisioningFailed {
                    phase: ChatProvisioningPhase::Credential,
                    command_index: None,
                    retryable: true,
                })
            })?
            .ok_or_else(|| {
                chat_unavailable(ChatUnavailableCause::ProvisioningFailed {
                    phase: ChatProvisioningPhase::Credential,
                    command_index: None,
                    retryable: false,
                })
            })?;
        let current = store
            .get(host_id)
            .await
            .map_err(|_| host_store_unavailable())?;
        if !current.as_ref().is_some_and(|current| {
            matches!(
                current.status,
                ChatHostStatus::Active | ChatHostStatus::Reviving
            ) && current.runtime_id.as_deref() == Some(runtime_id.as_str())
                && generation_for(current) == generation_for(&host)
        }) {
            return Err(chat_unavailable(ChatUnavailableCause::Provisioning));
        }
        adapter
            .refresh_credential(&RuntimeHandle(runtime_id.clone()), &credential)
            .await
            .map_err(|_| {
                chat_unavailable(ChatUnavailableCause::ProvisioningFailed {
                    phase: ChatProvisioningPhase::Credential,
                    command_index: None,
                    retryable: true,
                })
            })?;
        let confirmed = source
            .confirm_credential_delivery(
                &credential_session,
                &host.owner,
                &services,
                credential.expose(),
                &target,
            )
            .await
            .map_err(|_| {
                chat_unavailable(ChatUnavailableCause::ProvisioningFailed {
                    phase: ChatProvisioningPhase::Credential,
                    command_index: None,
                    retryable: true,
                })
            })?;
        if !confirmed {
            return Err(chat_unavailable(ChatUnavailableCause::ProvisioningFailed {
                phase: ChatProvisioningPhase::Credential,
                command_index: None,
                retryable: true,
            }));
        }
        let current = store
            .get(host_id)
            .await
            .map_err(|_| host_store_unavailable())?;
        if !current.as_ref().is_some_and(|current| {
            matches!(
                current.status,
                ChatHostStatus::Active | ChatHostStatus::Reviving
            ) && current.runtime_id.as_deref() == Some(runtime_id.as_str())
                && generation_for(current) == generation_for(&host)
        }) {
            return Err(chat_unavailable(ChatUnavailableCause::Provisioning));
        }
        Ok(())
    }

    /// Retire a host after every retained thread slot has been removed.
    pub(crate) async fn retire_chat_host_if_empty(
        &self,
        host_id: &ChatHostId,
    ) -> Result<ChatHostRecord, CoreError> {
        let store = self.chat_hosts.clone().ok_or_else(host_store_unavailable)?;
        let observed = store
            .get(host_id)
            .await
            .map_err(|_| host_store_unavailable())?
            .ok_or_else(|| chat_unavailable(ChatUnavailableCause::Retired))?;
        let _owner_guard = self.chat_host_guards.lock(&observed.owner).await;
        let mut host = store
            .get(host_id)
            .await
            .map_err(|_| host_store_unavailable())?
            .ok_or_else(|| chat_unavailable(ChatUnavailableCause::Retired))?;
        if host.status == ChatHostStatus::Retired {
            return Ok(host);
        }
        if host.threads.iter().any(|thread| !thread.removed) {
            return Ok(host);
        }

        host.status = ChatHostStatus::Pausing;
        host.last_transition_claim = Some(format!("chcl_{}", uuid::Uuid::new_v4().simple()));
        host.transition_started_at = Some(crate::clock::now_rfc3339());
        host.updated_at = crate::clock::now_rfc3339();
        let mut host = store
            .update_if_revision(host.revision, &host)
            .await
            .map_err(|_| host_store_unavailable())?
            .ok_or_else(|| chat_unavailable(ChatUnavailableCause::Provisioning))?;
        if !self
            .chat_hosts_starting
            .write()
            .await
            .insert(host.id.clone())
        {
            return Err(chat_unavailable(ChatUnavailableCause::Provisioning));
        }

        let result = async {
            if let Some(runtime_id) = host.runtime_id.clone() {
                let adapter = self.adapters.get(&host.kernel).cloned().ok_or_else(|| {
                    chat_unavailable(ChatUnavailableCause::CapabilityUnsupported(
                        "configured chat kernel".into(),
                    ))
                })?;
                let handle = RuntimeHandle(runtime_id.clone());
                // Every thread is already removed and the exact sandbox is the
                // cleanup boundary. Per-tree mutation would be redundant and
                // can deadlock retirement behind a failed checkpoint writer.
                let identity = RuntimeIdentity {
                    handle,
                    session: host.id.0.clone(),
                    generation: Some(generation_for(&host)),
                };
                if adapter.terminate_if_identity(&identity).await.is_err() {
                    return self.record_chat_retirement_failure(&host, true).await;
                }
                host.runtime_id = None;
            }

            if let Some(credentials) = self.credentials.as_ref() {
                if credentials
                    .release(&host.credential_session_id())
                    .await
                    .is_err()
                {
                    return self.record_chat_retirement_failure(&host, false).await;
                }
            }

            let now = crate::clock::now_rfc3339();
            host.status = ChatHostStatus::Retired;
            host.retired_at = Some(now.clone());
            host.last_transition_claim = None;
            host.transition_started_at = None;
            host.last_failure = None;
            host.updated_at = now;
            store
                .update_if_revision(host.revision, &host)
                .await
                .map_err(|_| host_store_unavailable())?
                .ok_or_else(|| chat_unavailable(ChatUnavailableCause::Provisioning))
        }
        .await;
        self.chat_hosts_starting.write().await.remove(host_id);
        result
    }

    async fn record_chat_retirement_failure(
        &self,
        observed: &ChatHostRecord,
        runtime_cleanup_pending: bool,
    ) -> Result<ChatHostRecord, CoreError> {
        let failure = ChatHostFailure {
            phase: if runtime_cleanup_pending {
                ChatProvisioningPhase::Cleanup
            } else {
                ChatProvisioningPhase::Credential
            },
            command_index: None,
            output_excerpt: None,
            retryable: true,
            cleanup_pending: runtime_cleanup_pending,
            occurred_at: crate::clock::now_rfc3339(),
        };
        if let Some(store) = self.chat_hosts.as_ref() {
            if let Ok(Some(mut current)) = store.get(&observed.id).await {
                if current.last_transition_claim == observed.last_transition_claim {
                    current.status = ChatHostStatus::Pausing;
                    current.last_failure = Some(failure.clone());
                    current.last_transition_claim = None;
                    current.transition_started_at = None;
                    current.updated_at = crate::clock::now_rfc3339();
                    if !runtime_cleanup_pending {
                        current.runtime_id = None;
                    }
                    let _ = store.update_if_revision(current.revision, &current).await;
                }
            }
        }
        Err(chat_unavailable(failure_cause(&failure)))
    }
}
