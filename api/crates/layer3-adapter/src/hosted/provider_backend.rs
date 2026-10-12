use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::deployment::{
    provider_metadata_identity, provider_request_identity, ProviderIdentity,
    CONTROL_PLANE_METADATA, SANDBOX_METADATA,
};
use super::{
    CheckpointArchiveStoreFactory, HostedAgentOpenSettings, HostedBackend,
    ProviderHostedControlTransport,
};
use crate::provider::{
    ProviderDeleteTarget, ProviderSandbox, ProviderSandboxRequest, SandboxClient,
};
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeHandle,
    RuntimeLifecycle, RuntimeObservation,
};

const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(180);
const DEFAULT_READY_POLL: Duration = Duration::from_millis(500);

/// Runtime control-link implementation paired with the provider lifecycle.
#[async_trait::async_trait]
pub trait HostedControlTransport: Send + Sync {
    async fn bind_runtime(&self, _handle: &RuntimeHandle, _sandbox_id: &str) -> crate::Result<()> {
        Ok(())
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame>;

    /// Revalidate a published checkpoint's writer hold without requiring export.
    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &crate::CheckpointHoldRequest,
    ) -> crate::Result<ResponseFrame> {
        self.exchange(handle, RequestFrame::CheckpointForTeardown(request.clone()))
            .await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)>;

    async fn retire(&self, handle: &RuntimeHandle) -> crate::Result<()>;
}

/// Production backend: provider lifecycle plus an injected supervisor link.
pub struct ProviderHostedBackend {
    client: SandboxClient,
    transport: Arc<dyn HostedControlTransport>,
    ready_timeout: Duration,
    ready_poll: Duration,
    created_by: String,
}

impl ProviderHostedBackend {
    pub fn new(
        client: SandboxClient,
        transport: Arc<dyn HostedControlTransport>,
        created_by: impl Into<String>,
    ) -> crate::Result<Self> {
        let created_by = created_by.into();
        if created_by.is_empty()
            || created_by.len() > 256
            || created_by.trim() != created_by
            || created_by.chars().any(char::is_control)
        {
            return Err(AdapterError::Rejected(
                "provider inventory requires a control-plane identity".into(),
            ));
        }
        Ok(Self {
            client,
            transport,
            ready_timeout: DEFAULT_READY_TIMEOUT,
            ready_poll: DEFAULT_READY_POLL,
            created_by,
        })
    }

    pub fn production(
        client: SandboxClient,
        archive_factory: Arc<dyn CheckpointArchiveStoreFactory>,
        created_by: impl Into<String>,
    ) -> crate::Result<Self> {
        let transport = ProviderHostedControlTransport::new(client.clone(), archive_factory)?;
        Self::new(client, Arc::new(transport), created_by)
    }

    #[must_use]
    pub fn with_readiness(mut self, timeout: Duration, poll: Duration) -> Self {
        self.ready_timeout = timeout;
        self.ready_poll = poll;
        self
    }

    pub fn client(&self) -> &SandboxClient {
        &self.client
    }

    fn require_creator(&self, request: &ProviderSandboxRequest) -> crate::Result<()> {
        if request.metadata.get(CONTROL_PLANE_METADATA) != Some(&self.created_by) {
            return Err(identity_mismatch());
        }
        Ok(())
    }

    async fn scoped_inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        creator_inventory(&self.created_by, self.client.list().await?)
    }

    async fn await_running(
        &self,
        request: &ProviderSandboxRequest,
        initial: crate::provider::ProviderSandbox,
    ) -> crate::Result<RuntimeHandle> {
        let handle = RuntimeHandle(initial.id.clone());
        if initial.is_running() {
            return Ok(handle);
        }
        let deadline = tokio::time::Instant::now() + self.ready_timeout;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(AdapterError::Unavailable(format!(
                    "sandbox {} did not start within {} seconds",
                    handle.0,
                    self.ready_timeout.as_secs()
                )));
            }
            tokio::time::sleep(self.ready_poll).await;
            let sandbox =
                validate_sandbox_identity(request, self.client.get(handle.as_str()).await?)?;
            if sandbox.is_running() {
                return Ok(handle);
            }
        }
    }

    async fn reconcile(
        &self,
        request: &ProviderSandboxRequest,
    ) -> crate::Result<Option<ProviderSandbox>> {
        self.require_creator(request)?;
        matching_sandbox(request, self.client.list().await?)
    }
}

#[async_trait::async_trait]
impl HostedBackend for ProviderHostedBackend {
    async fn provision_receipted(
        &self,
        mut request: ProviderSandboxRequest,
        operation_id: &str,
        request_sha256: &str,
    ) -> crate::Result<crate::RuntimeProvisionReceipt> {
        let reclaimer = self.client.finality_reclaimer()?;
        reclaimer.require_finality_scope()?;
        let sandbox_id = provider_request_identity(&request)?.sandbox_id;
        if self.reconcile(&request).await?.is_some() {
            return Err(AdapterError::Unavailable(
                "existing runtime cannot acquire a receipt for this dispatch".into(),
            ));
        }
        let marker = crate::provider::dispatch_marker(operation_id)?;
        let pre_dispatch = reclaimer.observe_before_dispatch(operation_id).await?;
        request
            .metadata
            .insert(crate::provider::DISPATCH_LABEL.to_owned(), marker.clone());
        // Exactly one POST. A failed or lost response cannot authorize a receipt.
        let (created, response_sha256) = self.client.create_receipted(&request).await?;
        let created = validate_sandbox_identity(&request, created)?;
        if request
            .metadata
            .iter()
            .any(|(key, value)| created.metadata.get(key) != Some(value))
        {
            return Err(identity_mismatch());
        }
        let handle = RuntimeHandle(created.id.clone());
        let original = reclaimer
            .capture_for_binding(
                &handle,
                operation_id,
                request_sha256,
                &response_sha256,
                &request.metadata,
                pre_dispatch,
            )
            .await?;
        let handle = self.await_running(&request, created).await?;
        reclaimer.require_bound_runtime(&original).await?;
        self.transport.bind_runtime(&handle, &sandbox_id).await?;
        Ok(original)
    }

    async fn finalize_runtime(
        &self,
        original: &crate::RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> crate::Result<crate::RuntimeFinalityReceipt> {
        let reclaimer = self.client.finality_reclaimer()?;
        reclaimer.require_bound_runtime(original).await?;
        self.transport.retire(original.handle()).await?;
        reclaimer
            .finalize_bound_runtime(original, operation_id)
            .await
    }

    async fn provision(&self, request: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        let sandbox_id = provider_request_identity(&request)?.sandbox_id;
        let sandbox = match self.reconcile(&request).await? {
            Some(sandbox) => sandbox,
            None => match self.client.create(&request).await {
                Ok(sandbox) => {
                    let created = validate_sandbox_identity(&request, sandbox)?;
                    match self.reconcile(&request).await? {
                        Some(reconciled) if reconciled.id == created.id => reconciled,
                        Some(_) => return Err(identity_mismatch()),
                        None => created,
                    }
                }
                Err(create_error) => match self.reconcile(&request).await? {
                    Some(sandbox) => sandbox,
                    None => return Err(create_error.into()),
                },
            },
        };
        let handle = self.await_running(&request, sandbox).await?;
        self.transport.bind_runtime(&handle, &sandbox_id).await?;
        Ok(handle)
    }

    async fn inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        self.scoped_inventory().await
    }

    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> crate::Result<Option<RuntimeObservation>> {
        self.require_creator(request)?;
        let expected = provider_request_identity(request)?;
        let observation = select_adoption(handle, &expected, self.scoped_inventory().await?)?;
        if observation.is_some() {
            self.transport
                .bind_runtime(handle, &expected.sandbox_id)
                .await?;
        }
        Ok(observation)
    }

    async fn reclaim(&self, observation: &RuntimeObservation) -> crate::Result<()> {
        let Some(target) = self
            .client
            .capture_delete_target(&observation.handle)
            .await?
        else {
            return Ok(());
        };
        let current = self
            .scoped_inventory()
            .await?
            .into_iter()
            .find(|current| current.handle == observation.handle);
        if current.as_ref() != Some(observation) {
            return Err(identity_mismatch());
        }
        self.transport.retire(&observation.handle).await?;
        self.client.delete_captured(target).await?;
        Ok(())
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        self.transport.exchange(handle, request).await
    }

    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &crate::CheckpointHoldRequest,
    ) -> crate::Result<ResponseFrame> {
        self.transport.verify_checkpoint_hold(handle, request).await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.transport
            .open_agent(handle, launch, resume_conversation, settings, updates)
            .await
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        // Capture immutable provider authority before waiting on local link
        // retirement. A reused provider name can never redirect this delete.
        let target: Option<ProviderDeleteTarget> =
            self.client.capture_delete_target(handle).await?;
        self.transport.retire(handle).await?;
        if let Some(target) = target {
            self.client.delete_captured(target).await?;
        }
        Ok(())
    }
}

fn select_adoption(
    handle: &RuntimeHandle,
    expected: &ProviderIdentity,
    inventory: Vec<RuntimeObservation>,
) -> crate::Result<Option<RuntimeObservation>> {
    let observation = inventory
        .into_iter()
        .find(|observation| observation.handle == *handle);
    let Some(observation) = observation else {
        return Ok(None);
    };
    if !observation_matches(&observation, expected) {
        return Err(identity_mismatch());
    }
    if !observation.lifecycle.is_running() {
        // Exact identity is proven above. Readiness can change while a pod is
        // replaced; leave durable recovery to reconciliation instead of failing
        // the lifecycle operation permanently.
        return Err(AdapterError::Unavailable(format!(
            "provider runtime {} is not running",
            handle.0
        )));
    }
    Ok(Some(observation))
}

fn matching_sandbox(
    request: &ProviderSandboxRequest,
    sandboxes: Vec<ProviderSandbox>,
) -> crate::Result<Option<ProviderSandbox>> {
    let identity = provider_request_identity(request)?;
    let mut candidates = Vec::new();
    for sandbox in sandboxes {
        if sandbox.metadata.get(SANDBOX_METADATA) == Some(&identity.sandbox_id)
            && sandbox.metadata.get(CONTROL_PLANE_METADATA)
                == request.metadata.get(CONTROL_PLANE_METADATA)
        {
            let candidate = provider_metadata_identity(&sandbox.metadata)?;
            if candidate.generation == identity.generation {
                candidates.push(sandbox);
            }
        }
    }
    let mut candidates = candidates.into_iter();
    let Some(sandbox) = candidates.next() else {
        return Ok(None);
    };
    if candidates.next().is_some() {
        return Err(identity_mismatch());
    }
    validate_sandbox_identity(request, sandbox).map(Some)
}

fn validate_sandbox_identity(
    request: &ProviderSandboxRequest,
    sandbox: ProviderSandbox,
) -> crate::Result<ProviderSandbox> {
    let identity = provider_request_identity(request)?;
    if invalid_handle(&sandbox.id)
        || provider_metadata_identity(&sandbox.metadata).as_ref() != Ok(&identity)
        || sandbox.metadata.get(CONTROL_PLANE_METADATA)
            != request.metadata.get(CONTROL_PLANE_METADATA)
    {
        return Err(identity_mismatch());
    }
    Ok(sandbox)
}

fn creator_inventory(
    created_by: &str,
    sandboxes: Vec<ProviderSandbox>,
) -> crate::Result<Vec<RuntimeObservation>> {
    // A shared provider also hosts generic environments and other services.
    // Only this controller's resources carry its runtime-contract obligations.
    // Filter ownership before decoding that contract; foreign images need no
    // CMA metadata and must never become candidates for orphan reclamation.
    let owned = sandboxes
        .into_iter()
        .filter(|sandbox| {
            sandbox
                .metadata
                .get(CONTROL_PLANE_METADATA)
                .map(String::as_str)
                == Some(created_by)
        })
        .collect();
    provider_inventory(owned)
}

fn provider_inventory(sandboxes: Vec<ProviderSandbox>) -> crate::Result<Vec<RuntimeObservation>> {
    let mut handles = BTreeSet::new();
    let mut generations = BTreeSet::new();
    let mut observations = Vec::with_capacity(sandboxes.len());
    for sandbox in sandboxes {
        if invalid_handle(&sandbox.id) {
            return Err(identity_mismatch());
        }
        let identity = provider_metadata_identity(&sandbox.metadata)?;
        let handle = RuntimeHandle(sandbox.id);
        if !handles.insert(handle.clone())
            || !generations.insert((identity.sandbox_id.clone(), identity.generation))
        {
            return Err(identity_mismatch());
        }
        let lifecycle = match sandbox.status {
            Some(status) if valid_status(&status.state, status.reason.as_deref()) => {
                RuntimeLifecycle {
                    state: if status.state.trim().is_empty() {
                        "unknown".into()
                    } else {
                        status.state
                    },
                    reason: status.reason,
                }
            }
            None => RuntimeLifecycle {
                state: "unknown".into(),
                reason: None,
            },
            Some(_) => {
                return Err(AdapterError::Rejected(
                    "provider sandbox has an invalid lifecycle status".into(),
                ));
            }
        };
        observations.push(RuntimeObservation {
            sandbox_id: identity.sandbox_id,
            generation: identity.generation,
            handle,
            image_sha256: identity.image_sha256,
            spec_fingerprint: identity.spec_fingerprint,
            runtime_contract: identity.runtime_contract,
            lifecycle,
        });
    }
    observations.sort_by(|left, right| {
        (&left.sandbox_id, left.generation, &left.handle).cmp(&(
            &right.sandbox_id,
            right.generation,
            &right.handle,
        ))
    });
    Ok(observations)
}

fn observation_matches(observation: &RuntimeObservation, expected: &ProviderIdentity) -> bool {
    observation.sandbox_id == expected.sandbox_id
        && observation.generation == expected.generation
        && observation.image_sha256 == expected.image_sha256
        && observation.spec_fingerprint == expected.spec_fingerprint
        && observation.runtime_contract == expected.runtime_contract
}

fn invalid_handle(value: &str) -> bool {
    value.trim().is_empty()
        || value.trim() != value
        || value.len() > 512
        || value.chars().any(char::is_control)
}

fn valid_status(state: &str, reason: Option<&str>) -> bool {
    state.len() <= 128
        && !state.chars().any(char::is_control)
        && reason.is_none_or(|reason| {
            reason.chars().count() <= 1_024
                && reason
                    .chars()
                    .all(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        })
}

fn identity_mismatch() -> AdapterError {
    AdapterError::Rejected(
        "provider sandbox identity is ambiguous or belongs to another specification".into(),
    )
}

#[cfg(test)]
#[path = "provider_backend_tests.rs"]
mod tests;
