//! Both consent interfaces share the durable owner workflow and command ledger.
use std::sync::Arc;

use axum::Router;
use layer2_core::{
    document_artifacts::DocumentService,
    resource_access::ResourceAccessService,
    sandbox::{
        agent_artifacts::AgentArtifactService, agent_filesystem::AgentFileService,
        agent_io::AgentCommandService, answer_deliverables::AnswerDeliverableService,
        AgentReadService, SandboxOperatorService, SandboxPolicySource, StoredSandboxRetirement,
    },
    ActivityService,
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
    pub ownership: Option<Arc<layer2_core::agent_ownership::OwnershipCoordinator>>,
    pub cancel_wake: Option<Arc<tokio::sync::Notify>>,
    pub model_credit_required: bool,
    pub model_funding: Option<Arc<layer2_core::sandbox::model_credit::AgentTurnFunding>>,
    pub source: Arc<MongoAgentAccessToolSource>,
    pub documents: Option<Arc<layer2_core::document_artifacts::DocumentService>>,
    pub deliverables: Arc<layer2_core::sandbox::answer_deliverables::AnswerDeliverableService>,
    pub public_origin: String,
    pub task_experience: Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>,
    pub github_connection_approval: bool,
    pub runtime_adapter: Arc<dyn SandboxAdapter>,
}

pub(super) fn sandbox_control(
    stores: &MongoStores,
    access: Arc<ResourceAccessService>,
    auth: Arc<AuthState>,
    connections: Arc<crate::connections::ConnectionPrerequisite>,
) -> (
    crate::commands::SandboxControlScheduler,
    Arc<layer2_core::sandbox::control::SandboxControlService>,
) {
    use layer2_core::sandbox::control::{SandboxControlService, SandboxControlWorker};
    let store = stores.sandbox_model.clone();
    let worker =
        SandboxControlWorker::new(store.clone(), access.clone(), auth.clone(), connections);
    let service = SandboxControlService::new(store.clone(), access, auth);
    service.attach_platform_spend(store);
    (
        crate::commands::SandboxControlScheduler::new(worker),
        service,
    )
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
    .with_task_store(stores.sandbox_model.clone())
}

pub(super) fn compose(
    stores: &MongoStores,
    auth: Arc<AuthState>,
    policy: Arc<dyn SandboxPolicySource>,
    broker: Arc<crate::trigger::CmaServiceConnectionBroker>,
    resource_access: Arc<ResourceAccessService>,
    holder: String,
    tools: ToolServices,
) -> AccessComposition {
    let commands =
        AgentCommandService::new(stores.sandbox_model.clone(), resource_access, auth.clone())
            .with_task_experience(tools.task_experience)
            .with_ownership(tools.ownership)
            .with_cancel_wake(tools.cancel_wake)
            .with_model_credit_required(tools.model_credit_required)
            .with_model_funding(tools.model_funding);
    let service = AgentAccessService::new(stores.sandbox_model.clone(), auth.clone(), policy);
    let service = if tools.github_connection_approval {
        service.with_github_approval(auth.clone(), broker.clone())
    } else {
        service
    };
    let worker = AgentAccessWorker::new(
        stores.sandbox_model.clone(),
        auth,
        broker.clone(),
        commands.clone(),
        holder,
    )
    .with_github_approval(broker, tools.runtime_adapter);
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

pub(super) fn sandbox_operators(
    stores: &MongoStores,
    activity: &Arc<ActivityService>,
    features: crate::config::FeatureFlags,
) -> Arc<SandboxOperatorService> {
    let retirement = Arc::new(StoredSandboxRetirement::new(stores.sandbox_model.clone()));
    Arc::new(
        SandboxOperatorService::new(
            stores.sandbox_model.clone(),
            stores.sandbox_profiles.clone(),
            retirement,
            Arc::clone(activity),
        )
        .with_maintenance(stores.sandbox_model.clone())
        .with_reviewed_retirement(
            stores.sandbox_model.clone(),
            features.reviewed_sandbox_retirement_enabled,
        ),
    )
}
