//! Durable ownership and lifecycle state for one subject's shared chat runtime.

use std::collections::HashSet;
use std::num::NonZeroU64;
use std::sync::Arc;

use layer3_adapter::{
    AdapterError, RuntimeEnvironment, RuntimeHandle, RuntimeIdentity, RuntimeMode, RuntimeOptions,
    RuntimePermission, RuntimeSpec, TermSize, WorkspaceSourceRequest,
};
use serde::{Deserialize, Serialize};

use crate::store::{PageRequest, MAX_PAGE_SIZE};
use crate::{
    ChatCheckpointRef, ChatSettingsSnapshot, CoreError, CredentialDeliveryTarget, KernelId,
    SessionId, SubjectId,
};

const CHAT_HOST_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x1fb7_66ee_28df_5e10_a833_2697_9a23_f36d);

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
    /// receiving a deterministic identity that no user session can mint.
    pub(crate) fn credential_session_id(&self) -> SessionId {
        SessionId(format!("sesn_chat_{}", self.0.trim_start_matches("chho_")))
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
    pub retryable: bool,
    pub cleanup_pending: bool,
    pub occurred_at: String,
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
    pub threads: Vec<ChatThreadSlot>,
    pub threads_created: u32,
    pub checkpoint: Option<ChatCheckpointRef>,
    pub paused_at: Option<String>,
    pub revived_at: Option<String>,
    pub retired_at: Option<String>,
    pub last_transition_claim: Option<String>,
    /// Start of the current claim, retained for diagnostics and later leases.
    pub transition_started_at: Option<String>,
    pub last_failure: Option<ChatHostFailure>,
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
            threads: Vec::new(),
            threads_created: 0,
            checkpoint: None,
            paused_at: None,
            revived_at: None,
            retired_at: None,
            last_transition_claim: Some(claim),
            transition_started_at: Some(now.clone()),
            last_failure: None,
            rotations: 0,
            created_at: now.clone(),
            updated_at: now,
            revision: 0,
        }
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
        Ok(())
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

fn generation_for(host: &ChatHostRecord) -> NonZeroU64 {
    NonZeroU64::new(u64::from(host.rotations).saturating_add(1))
        .expect("a chat host generation is always non-zero")
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

    /// Return this owner's one usable shared chat host, provisioning it once
    /// when no durable host exists.
    pub(crate) async fn ensure_chat_host(
        self: &Arc<Self>,
        owner: &SubjectId,
        caller_credential: Option<&crate::CallerCredential>,
    ) -> Result<ChatHostRecord, CoreError> {
        let store = self.chat_hosts.clone().ok_or_else(host_store_unavailable)?;
        let _owner_guard = self.chat_host_guards.lock(owner).await;

        let existing = store
            .get_for_owner(owner)
            .await
            .map_err(|_| host_store_unavailable())?;
        let host = match existing {
            Some(host) if host.status == ChatHostStatus::Active => return Ok(host),
            Some(host) if host.status == ChatHostStatus::Retired => {
                return Err(chat_unavailable(ChatUnavailableCause::Retired));
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
                let candidate = ChatHostRecord::new(
                    owner.clone(),
                    policy.snapshot,
                    policy.briefing.render(&service_names),
                    kernel,
                    now,
                    format!("chcl_{}", uuid::Uuid::new_v4().simple()),
                );
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

        let credential_session = host.id.credential_session_id();
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
            reservation_token: None,
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
        };
        if let Err(error) = adapter.prepare_workspace_source(&handle, &source).await {
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
        if let Some(failed) = setup.steps.iter().find(|step| step.exit_code != 0) {
            return self
                .fail_chat_host(
                    &host,
                    Some((&handle, &adapter)),
                    ChatProvisioningPhase::WorkspaceSetup,
                    Some(failed.command_index),
                    true,
                    None,
                )
                .await;
        }

        host.status = ChatHostStatus::Active;
        host.runtime_version = self.runtime_version.clone();
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
        command_index: Option<usize>,
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
            command_index,
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
        if host.status != ChatHostStatus::Active {
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
        let credential_session = host.id.credential_session_id();
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
            current.revision == host.revision
                && current.status == ChatHostStatus::Active
                && current.runtime_id.as_deref() == Some(runtime_id.as_str())
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
            current.status == ChatHostStatus::Active
                && current.runtime_id.as_deref() == Some(runtime_id.as_str())
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
                for thread in &host.threads {
                    if let Err(error) = adapter
                        .remove_workspace_tree(&handle, &thread.worktree_name)
                        .await
                    {
                        tracing::warn!(
                            chat_host = %host.id,
                            worktree = %thread.worktree_name,
                            %error,
                            "chat worktree cleanup was not confirmed before retirement"
                        );
                    }
                }
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
                    .release(&host.id.credential_session_id())
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
