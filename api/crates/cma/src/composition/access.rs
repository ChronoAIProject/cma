//! Both consent interfaces share the durable owner workflow and command ledger.
use std::sync::Arc;

use axum::Router;
use layer2_core::{
    document_artifacts::DocumentService,
    resource_access::ResourceAccessService,
    sandbox::{
        agent_artifacts::AgentArtifactService, agent_filesystem::AgentFileService,
        agent_io::AgentCommandService, answer_deliverables::AnswerDeliverableService,
        AgentReadService, SandboxPolicySource,
    },
    trigger_domain::ServiceConnectionBroker,
};
use layer3_adapter::SandboxAdapter;

use crate::{
    agent_access::AgentAccessService,
    agent_access_worker::AgentAccessWorker,
    auth::AuthState,
    store::{MongoAgentAccessToolSource, MongoStores},
};

pub(super) struct AccessComposition {
    pub commands: Arc<AgentCommandService>,
    pub worker: AgentAccessWorker,
    pub owner_routes: Router,
    pub tool_routes: Router,
}
pub(super) struct ToolServices {
    pub source: Arc<MongoAgentAccessToolSource>,
    pub documents: Option<Arc<layer2_core::document_artifacts::DocumentService>>,
    pub deliverables: Arc<layer2_core::sandbox::answer_deliverables::AnswerDeliverableService>,
    pub public_origin: String,
}

pub(super) fn deliverables(
    stores: &MongoStores,
    reads: Arc<AgentReadService>,
    access: Arc<ResourceAccessService>,
    documents: Arc<DocumentService>,
    adapter: Arc<dyn SandboxAdapter>,
) -> Arc<AnswerDeliverableService> {
    AnswerDeliverableService::new(
        reads,
        AgentArtifactService::new(stores.sandbox_model.clone(), access.clone()),
    )
    .with_documents(documents)
    .with_files(AgentFileService::new(access, adapter))
}

pub(super) fn compose(
    stores: &MongoStores,
    auth: Arc<AuthState>,
    policy: Arc<dyn SandboxPolicySource>,
    broker: Arc<dyn ServiceConnectionBroker>,
    resource_access: Arc<ResourceAccessService>,
    holder: String,
    tools: ToolServices,
) -> AccessComposition {
    let commands =
        AgentCommandService::new(stores.sandbox_model.clone(), resource_access, auth.clone());
    let service = AgentAccessService::new(stores.sandbox_model.clone(), auth.clone(), policy);
    let worker = AgentAccessWorker::new(
        stores.sandbox_model.clone(),
        auth,
        broker,
        commands.clone(),
        holder,
    );
    AccessComposition {
        commands,
        worker,
        owner_routes: layer1_public::rest::agent_access_requests_router(service.clone()),
        // Tool capabilities authenticate independently from human browser auth.
        tool_routes: layer1_public::mcp::agent_delivery_tools_router(
            tools.source,
            service,
            tools.documents,
            Some(tools.deliverables),
            &tools.public_origin,
        ),
    }
}
