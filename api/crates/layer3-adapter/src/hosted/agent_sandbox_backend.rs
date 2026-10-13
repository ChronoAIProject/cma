use super::{HostedAgentOpenSettings, HostedBackend, HostedControlTransport};
use crate::{
    agent_sandbox::AgentSandboxClient,
    provider::ProviderSandboxRequest,
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    *,
};
use kube::{api::DynamicObject, ResourceExt};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;

pub struct AgentSandboxBackend {
    client: AgentSandboxClient,
    transport: Arc<dyn HostedControlTransport>,
}

impl AgentSandboxBackend {
    pub fn new(client: AgentSandboxClient, transport: Arc<dyn HostedControlTransport>) -> Self {
        Self { client, transport }
    }

    async fn bind(&self, handle: &RuntimeHandle, request: &ProviderSandboxRequest) -> Result<()> {
        self.client
            .wait_ready(handle, Duration::from_secs(180))
            .await?;
        let identity = super::deployment::provider_request_identity(request)?;
        self.transport
            .bind_runtime(handle, &identity.sandbox_id)
            .await
    }
}

fn observation(object: &DynamicObject) -> Result<RuntimeObservation> {
    let identity = super::deployment::provider_metadata_identity(object.annotations())?;
    let ready = object
        .data
        .pointer("/status/conditions")
        .and_then(|v| v.as_array())
        .is_some_and(|cs| {
            cs.iter().any(|c| {
                c["type"] == "Ready"
                    && c["status"] == "True"
                    && c["observedGeneration"].as_i64() == object.metadata.generation
            })
        });
    let state = if object.metadata.deletion_timestamp.is_some() {
        "terminating"
    } else if object
        .data
        .pointer("/spec/operatingMode")
        .and_then(|value| value.as_str())
        == Some("Suspended")
    {
        "suspended"
    } else if ready {
        "running"
    } else {
        "pending"
    };
    Ok(RuntimeObservation {
        sandbox_id: identity.sandbox_id,
        generation: identity.generation,
        handle: crate::agent_sandbox::handle(object)?,
        image_sha256: identity.image_sha256,
        spec_fingerprint: identity.spec_fingerprint,
        runtime_contract: identity.runtime_contract,
        lifecycle: RuntimeLifecycle {
            state: state.into(),
            reason: None,
        },
    })
}

#[async_trait::async_trait]
impl HostedBackend for AgentSandboxBackend {
    async fn provision(&self, request: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        let handle = self.client.create(&request).await?;
        self.bind(&handle, &request).await?;
        Ok(handle)
    }
    async fn teardown_retained(&self, handle: &RuntimeHandle) -> Result<()> {
        self.teardown(handle).await
    }
    async fn suspend(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.client.suspend(handle, request).await?;
        self.transport.retire(handle).await
    }
    async fn verify_suspended(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.client.verify_suspended(handle, request).await
    }
    async fn resume(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.client.resume(handle, previous, next).await?;
        self.bind(handle, next).await
    }
    async fn provision_receipted(
        &self,
        request: ProviderSandboxRequest,
        operation_id: &str,
        request_sha256: &str,
    ) -> Result<RuntimeProvisionReceipt> {
        let receipt = self
            .client
            .create_receipted(&request, operation_id, request_sha256)
            .await?;
        self.bind(receipt.handle(), &request).await?;
        Ok(receipt)
    }
    async fn finalize_runtime(
        &self,
        original: &RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> Result<RuntimeFinalityReceipt> {
        self.transport.retire(original.handle()).await?;
        self.client.finalize(original, operation_id).await
    }
    async fn inventory(&self) -> Result<Vec<RuntimeObservation>> {
        self.client
            .inventory()
            .await?
            .iter()
            .map(observation)
            .collect()
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<Option<RuntimeObservation>> {
        let Some(object) = self.client.validate_request(handle, request).await? else {
            return Ok(None);
        };
        self.bind(handle, request).await?;
        Ok(Some(observation(&object)?))
    }
    async fn reclaim(&self, expected: &RuntimeObservation) -> Result<()> {
        let Some(object) = self.client.get(&expected.handle).await? else {
            return Ok(());
        };
        if observation(&object)? != *expected {
            return Err(AdapterError::Rejected(
                "Sandbox changed before reclamation".into(),
            ));
        }
        self.teardown(&expected.handle).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> Result<ResponseFrame> {
        self.transport.exchange(handle, request).await
    }

    async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> Result<Vec<u8>> {
        self.transport.export_agent_history(handle, agent).await
    }
    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
    ) -> Result<ResponseFrame> {
        self.transport.verify_checkpoint_hold(handle, request).await
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.transport
            .open_agent(handle, launch, resume_conversation, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()> {
        self.transport.retire(handle).await?;
        self.client.delete(handle).await
    }
}

/// Persistence port for the provider chosen before a runtime creation request.
#[async_trait::async_trait]
pub trait RuntimeRoutingStore: Send + Sync {
    async fn select(&self, sandbox: &str, generation: u64, default_new: bool) -> Result<bool>;
}

/// New allocations use the selected provider. Existing runtime handles retain
/// their provider during rollout and rollback; inventory includes both owners.
pub struct RuntimeBackends {
    pub agent_sandbox: Option<Arc<dyn HostedBackend>>,
    pub opensandbox: Option<Arc<dyn HostedBackend>>,
    pub allocate_agent_sandbox: bool,
    pub routing: Arc<dyn RuntimeRoutingStore>,
}

impl RuntimeBackends {
    fn select(&self, agent_sandbox: bool) -> Result<&dyn HostedBackend> {
        let backend = if agent_sandbox {
            &self.agent_sandbox
        } else {
            &self.opensandbox
        };
        backend.as_deref().ok_or_else(|| {
            AdapterError::Unavailable("recorded runtime provider is not configured".into())
        })
    }
    fn for_handle(&self, handle: &RuntimeHandle) -> Result<&dyn HostedBackend> {
        self.select(handle.0.starts_with("agent-sandbox:"))
    }
}

#[async_trait::async_trait]
impl HostedBackend for RuntimeBackends {
    async fn provision(&self, request: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        let identity = super::deployment::provider_request_identity(&request)?;
        let selected = self
            .routing
            .select(
                &identity.sandbox_id,
                identity.generation,
                self.allocate_agent_sandbox,
            )
            .await?;
        self.select(selected)?.provision(request).await
    }
    async fn teardown_retained(&self, handle: &RuntimeHandle) -> Result<()> {
        self.for_handle(handle)?.teardown_retained(handle).await
    }
    async fn suspend(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.for_handle(handle)?.suspend(handle, request).await
    }
    async fn verify_suspended(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.for_handle(handle)?
            .verify_suspended(handle, request)
            .await
    }
    async fn resume(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.for_handle(handle)?
            .resume(handle, previous, next)
            .await
    }
    async fn provision_receipted(
        &self,
        request: ProviderSandboxRequest,
        op: &str,
        digest: &str,
    ) -> Result<RuntimeProvisionReceipt> {
        let identity = super::deployment::provider_request_identity(&request)?;
        let selected = self
            .routing
            .select(
                &identity.sandbox_id,
                identity.generation,
                self.allocate_agent_sandbox,
            )
            .await?;
        self.select(selected)?
            .provision_receipted(request, op, digest)
            .await
    }
    async fn finalize_runtime(
        &self,
        original: &RuntimeProvisionReceipt,
        op: &str,
    ) -> Result<RuntimeFinalityReceipt> {
        self.for_handle(original.handle())?
            .finalize_runtime(original, op)
            .await
    }
    async fn inventory(&self) -> Result<Vec<RuntimeObservation>> {
        let mut all = Vec::new();
        for backend in [&self.agent_sandbox, &self.opensandbox]
            .into_iter()
            .flatten()
        {
            all.extend(backend.inventory().await?);
        }
        Ok(all)
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<Option<RuntimeObservation>> {
        self.for_handle(handle)?.adopt(handle, request).await
    }
    async fn reclaim(&self, observation: &RuntimeObservation) -> Result<()> {
        self.for_handle(&observation.handle)?
            .reclaim(observation)
            .await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> Result<ResponseFrame> {
        self.for_handle(handle)?.exchange(handle, request).await
    }
    async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> Result<Vec<u8>> {
        self.for_handle(handle)?
            .export_agent_history(handle, agent)
            .await
    }
    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
    ) -> Result<ResponseFrame> {
        self.for_handle(handle)?
            .verify_checkpoint_hold(handle, request)
            .await
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.for_handle(handle)?
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()> {
        self.for_handle(handle)?.teardown(handle).await
    }
}

#[cfg(test)]
#[path = "runtime_history_tests.rs"]
mod history_tests;

#[cfg(test)]
mod routing_tests {
    use super::*;
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Routes(tokio::sync::Mutex<BTreeMap<(String, u64), bool>>);
    #[async_trait::async_trait]
    impl RuntimeRoutingStore for Routes {
        async fn select(&self, id: &str, generation: u64, default_new: bool) -> Result<bool> {
            Ok(*self
                .0
                .lock()
                .await
                .entry((id.into(), generation))
                .or_insert(default_new))
        }
    }
    #[tokio::test]
    async fn unknown_creation_retry_keeps_original_backend_across_default_change() {
        let old = Arc::new(super::super::InMemoryHostedBackend::default());
        let new = Arc::new(super::super::InMemoryHostedBackend::default());
        let routes = Arc::new(Routes::default());
        let mut backend = RuntimeBackends {
            agent_sandbox: Some(new.clone()),
            opensandbox: Some(old.clone()),
            allocate_agent_sandbox: false,
            routing: routes,
        };
        let spec = SandboxSpec {
            sandbox_id: "owned-runtime".into(),
            created_by: Some("deployment".into()),
            ..Default::default()
        };
        let request = super::super::HostedDeployment::default()
            .render(&spec)
            .unwrap();
        let original = backend.provision(request.clone()).await.unwrap();
        backend.allocate_agent_sandbox = true;
        assert_eq!(original, backend.provision(request.clone()).await.unwrap());
        assert_eq!(old.inventory().await.unwrap().len(), 1);
        assert!(new.inventory().await.unwrap().is_empty());
        let next = super::super::HostedDeployment::default()
            .render_runtime(&spec, 2)
            .unwrap();
        backend.provision(next).await.unwrap();
        assert_eq!(new.inventory().await.unwrap().len(), 1);
    }
}
