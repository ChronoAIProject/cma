use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::deployment::{
    provider_metadata_identity, provider_request_identity, ProviderIdentity, SANDBOX_METADATA,
};
use super::HostedBackend;
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
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame>;

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
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
}

impl ProviderHostedBackend {
    pub fn new(client: SandboxClient, transport: Arc<dyn HostedControlTransport>) -> Self {
        Self {
            client,
            transport,
            ready_timeout: DEFAULT_READY_TIMEOUT,
            ready_poll: DEFAULT_READY_POLL,
        }
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
        matching_sandbox(request, self.client.list().await?)
    }
}

#[async_trait::async_trait]
impl HostedBackend for ProviderHostedBackend {
    async fn provision(&self, request: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
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
        self.await_running(&request, sandbox).await
    }

    async fn inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        provider_inventory(self.client.list().await?)
    }

    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> crate::Result<Option<RuntimeObservation>> {
        let expected = provider_request_identity(request)?;
        select_adoption(
            handle,
            &expected,
            provider_inventory(self.client.list().await?)?,
        )
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        self.transport.exchange(handle, request).await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.transport
            .open_agent(handle, launch, resume_conversation, updates)
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
        return Err(AdapterError::Rejected(format!(
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
        if sandbox.metadata.get(SANDBOX_METADATA) == Some(&identity.sandbox_id) {
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
    {
        return Err(identity_mismatch());
    }
    Ok(sandbox)
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
                ))
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
