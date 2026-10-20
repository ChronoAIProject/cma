//! Shared production composition for Agent-facing public projections.

use std::sync::Arc;

use axum::Router;
use layer1_public::rest::{agent_router, AgentRoutesState};
use layer1_public::rest::{
    AgentHistoryExportError, AgentHistoryExporter, AgentLifecycleOperationReader,
    AgentLifecycleProvisioner,
};
use layer2_core::profiles::{AgentProfileStore, ProfileUseRecorder};
use layer2_core::sandbox::store::{AgentReadStore, AgentServiceStore};
use layer2_core::sandbox::{
    AgentContextService, AgentOperationService, AgentOperationStore, AgentReadService,
    AgentRuntimeRegistry, AgentService, AgentUpdateRecorder, Sandbox, SandboxResourceReadError,
    SandboxResourceReader, SandboxResourceReading,
};
use layer2_core::{OperationStore, RuntimeCapabilities};
use layer3_adapter::{AdapterError, RuntimeHandle, SandboxAdapter};
#[path = "projections/agent_v2.rs"]
mod agent_v2;

pub(crate) struct AgentProjectionServices {
    message_feedback: Option<Arc<dyn layer2_core::sandbox::message_feedback::MessageFeedbackStore>>,
    skill_support: Option<Arc<dyn layer1_public::rest::AgentSkillRuntimeSupport>>,
    commands: Option<Arc<layer2_core::sandbox::agent_io::AgentCommandService>>,
    agents: Arc<AgentService>,
    operations: Arc<AgentOperationService>,
    reads: Arc<AgentReadService>,
    lifecycle: Option<Arc<dyn AgentLifecycleProvisioner>>,
    lifecycle_operations: Option<Arc<dyn AgentLifecycleOperationReader>>,
    history_exporter: Option<Arc<dyn AgentHistoryExporter>>,
}

impl AgentProjectionServices {
    pub(crate) fn with_message_feedback(
        mut self,
        store: Arc<dyn layer2_core::sandbox::message_feedback::MessageFeedbackStore>,
    ) -> Self {
        self.message_feedback = Some(store);
        self
    }

    pub(crate) fn with_skill_support(
        mut self,
        support: Arc<dyn layer1_public::rest::AgentSkillRuntimeSupport>,
    ) -> Self {
        self.skill_support = Some(support);
        self
    }

    pub(crate) fn with_commands(
        mut self,
        commands: Arc<layer2_core::sandbox::agent_io::AgentCommandService>,
    ) -> Self {
        self.commands = Some(commands);
        self
    }

    pub(crate) fn with_lifecycle(mut self, lifecycle: Arc<dyn AgentLifecycleProvisioner>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    pub(crate) fn with_lifecycle_operations(
        mut self,
        reader: Arc<dyn AgentLifecycleOperationReader>,
    ) -> Self {
        self.lifecycle_operations = Some(reader);
        self
    }

    pub(crate) fn with_history_exporter(mut self, exporter: Arc<dyn AgentHistoryExporter>) -> Self {
        self.history_exporter = Some(exporter);
        self
    }

    pub(crate) fn with_resource_access(
        mut self,
        access: Arc<layer2_core::resource_access::ResourceAccessService>,
    ) -> Self {
        self.reads = self.reads.with_resource_access(access);
        self
    }

    pub(crate) fn with_connections(
        mut self,
        connections: Arc<dyn layer2_core::sandbox::SandboxCredentialPrerequisite>,
    ) -> Self {
        self.agents = self.agents.with_connections(connections.clone());
        self.operations = self.operations.with_connections(connections);
        self
    }

    pub(crate) fn new<S, P, R, Q>(
        store: Arc<S>,
        profiles: Arc<P>,
        runtimes: Arc<R>,
        resources: Arc<Q>,
        runtime_capabilities: RuntimeCapabilities,
    ) -> Self
    where
        S: AgentServiceStore + AgentReadStore + AgentOperationStore + 'static,
        P: AgentProfileStore + ProfileUseRecorder + 'static,
        R: AgentRuntimeRegistry + 'static,
        Q: SandboxResourceReader + 'static,
    {
        let agents = AgentService::new(
            store.clone(),
            profiles.clone(),
            runtimes.clone(),
            runtime_capabilities,
        );
        let reads = AgentReadService::new(store.clone(), resources);
        let operations = AgentOperationService::new(store, runtimes);
        Self {
            message_feedback: None,
            skill_support: None,
            commands: None,
            agents,
            operations,
            reads,
            lifecycle: None,
            lifecycle_operations: None,
            history_exporter: None,
        }
    }

    pub(crate) fn agents(&self) -> Arc<AgentService> {
        Arc::clone(&self.agents)
    }

    pub(crate) fn operations(&self) -> Arc<AgentOperationService> {
        Arc::clone(&self.operations)
    }

    pub(crate) fn reads(&self) -> Arc<AgentReadService> {
        Arc::clone(&self.reads)
    }

    pub(crate) fn router(&self, operation_records: Arc<dyn OperationStore>) -> Router {
        let agent_routes = AgentRoutesState::new(
            Arc::clone(&self.agents),
            Arc::clone(&self.reads),
            Arc::clone(&self.operations),
            operation_records,
        );
        let agent_routes = match &self.message_feedback {
            Some(store) => agent_routes.with_message_feedback(store.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.skill_support {
            Some(support) => agent_routes.with_skill_support(support.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.lifecycle {
            Some(lifecycle) => agent_routes.with_lifecycle(lifecycle.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.lifecycle_operations {
            Some(reader) => agent_routes.with_lifecycle_operations(reader.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.history_exporter {
            Some(exporter) => agent_routes.with_history_exporter(exporter.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.commands {
            Some(commands) => agent_routes.with_commands(commands.clone()),
            None => agent_routes,
        };
        Router::new()
            .merge(agent_router(agent_routes))
            .merge(layer1_public::pty::router(self.reads.clone()))
    }

    pub(crate) fn conversation_router(
        &self,
        context: Arc<AgentContextService>,
        updates: Arc<dyn AgentUpdateRecorder>,
        public_base: &str,
    ) -> Router {
        let state = layer1_public::AgentConversationState::new(
            self.reads.clone(),
            self.operations.clone(),
            context,
        )
        .with_updates(updates);
        let state = match &self.commands {
            Some(commands) => state.with_commands(commands.clone()),
            None => state,
        };
        layer1_public::agui::router(state.clone())
            .merge(layer1_public::responses::router(state))
            .merge(layer1_public::mcp::router())
            .merge(layer1_public::a2a::router(public_base))
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct UnavailableSandboxResources;

#[cfg(test)]
#[async_trait::async_trait]
impl SandboxResourceReader for UnavailableSandboxResources {
    async fn sandbox_resources(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        Err(SandboxResourceReadError::Unavailable(
            "the hosted Sandbox resource transport is not composed".into(),
        ))
    }
}

pub(crate) struct AdapterSandboxResources {
    adapter: Arc<dyn SandboxAdapter>,
}

pub(crate) struct AdapterAgentHistoryExporter {
    adapter: Arc<dyn SandboxAdapter>,
}

impl AdapterAgentHistoryExporter {
    pub(crate) fn new(adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self { adapter }
    }
}

#[async_trait::async_trait]
impl AgentHistoryExporter for AdapterAgentHistoryExporter {
    async fn export(
        &self,
        runtime_id: &str,
        agent_id: &str,
    ) -> Result<Vec<u8>, AgentHistoryExportError> {
        let bytes = self
            .adapter
            .export_agent_history(
                &RuntimeHandle::new(runtime_id),
                &layer3_adapter::AgentSlot::new(agent_id),
            )
            .await
            .map_err(|error| match error {
                AdapterError::Unsupported(_) => AgentHistoryExportError::Unsupported,
                AdapterError::Unavailable(_) | AdapterError::AtCapacity(_) => {
                    AgentHistoryExportError::Unavailable
                }
                AdapterError::Rejected(_)
                | AdapterError::OperationIdentityConflict(_)
                | AdapterError::RuntimeContractMismatch(_) => AgentHistoryExportError::Rejected,
            })?;
        Ok(bytes)
    }
}

impl AdapterSandboxResources {
    pub(crate) fn new(adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self { adapter }
    }
}

#[async_trait::async_trait]
impl SandboxResourceReader for AdapterSandboxResources {
    async fn sandbox_resources(
        &self,
        sandbox: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        let runtime = sandbox.runtime.as_ref().ok_or_else(|| {
            SandboxResourceReadError::Unsupported("the sandbox has no active runtime".into())
        })?;
        if runtime.runtime_id.trim().is_empty() || runtime.runtime_id != runtime.runtime_id.trim() {
            return Err(SandboxResourceReadError::Corrupt(
                "the sandbox runtime identity is invalid".into(),
            ));
        }
        let reading = self
            .adapter
            .resources(&RuntimeHandle(runtime.runtime_id.clone()))
            .await
            .map_err(|error| resource_failure(&sandbox.id.to_string(), error))?;
        Ok(SandboxResourceReading {
            uptime_ms: reading.uptime_ms,
            cpu_usage_usec: reading.cpu_usage_usec,
            cpu_millis_limit: reading.cpu_millis_limit,
            memory_bytes_used: reading.memory_bytes_used,
            memory_bytes_limit: reading.memory_bytes_limit,
            memory_bytes_peak: reading.memory_bytes_peak,
            stall_permille: reading.stall_permille,
            ceiling_hits: reading.ceiling_hits,
            allocation_failures: reading.allocation_failures,
            kills: reading.kills,
            received_bytes: reading.received_bytes,
            sent_bytes: reading.sent_bytes,
        })
    }
}

fn resource_failure(sandbox_id: &str, error: AdapterError) -> SandboxResourceReadError {
    tracing::error!(sandbox_id, error = %error, "sandbox resource read failed");
    match error {
        AdapterError::Unsupported(_) => SandboxResourceReadError::Unsupported(
            "the runtime does not support resource readings".into(),
        ),
        AdapterError::Rejected(_)
        | AdapterError::OperationIdentityConflict(_)
        | AdapterError::RuntimeContractMismatch(_) => SandboxResourceReadError::Corrupt(
            "the runtime resource authority or reply is invalid".into(),
        ),
        AdapterError::Unavailable(_) | AdapterError::AtCapacity(_) => {
            SandboxResourceReadError::Unavailable(
                "the runtime resource transport is unavailable".into(),
            )
        }
    }
}

pub(crate) struct AdapterAgentSkillSupport(pub Arc<dyn SandboxAdapter>);
#[async_trait::async_trait]
impl layer1_public::rest::AgentSkillRuntimeSupport for AdapterAgentSkillSupport {
    async fn supported(&self, sandbox: &Sandbox) -> bool {
        let Some(binding) = &sandbox.runtime else {
            return false;
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.0
                .runtime_status(&RuntimeHandle::new(&binding.runtime_id)),
        )
        .await;
        match result {
            Ok(Ok(status)) => {
                status.agent_skill_install
                    && status.installation_ready
                    && status.credential_ready
                    && sandbox.runtime_process_matches(&status).unwrap_or(false)
            }
            _ => false,
        }
    }
}

pub(crate) struct AdapterAgentModelSupport(pub Arc<dyn SandboxAdapter>);
#[async_trait::async_trait]
impl layer1_public::rest::AgentModelRuntimeSupport for AdapterAgentModelSupport {
    async fn supported(&self, sandbox: &Sandbox) -> bool {
        let Some(binding) = &sandbox.runtime else {
            return false;
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.0
                .runtime_status(&RuntimeHandle::new(&binding.runtime_id)),
        )
        .await;
        match result {
            Ok(Ok(status)) => {
                status.agent_model_catalog
                    && status.installation_ready
                    && status.credential_ready
                    && sandbox.runtime_process_matches(&status).unwrap_or(false)
            }
            _ => false,
        }
    }
}
