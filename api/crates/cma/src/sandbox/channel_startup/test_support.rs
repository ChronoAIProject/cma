use super::*;
use crate::sandbox::{
    agent_skills::EmptyAgentSkillResolver,
    agent_worker::{AgentWorker, AgentWorkerConfig, AgentWorkerDependencies},
    live_agents::LiveAgentRegistry,
};
use layer2_core::sandbox::{AgentOperationService, DurableAgentUpdateRecorder};

pub(crate) async fn publish_connection(
    resources: Arc<crate::store::MongoSandboxStore>,
    adapter: Arc<dyn SandboxAdapter>,
    agent: &layer2_core::AgentId,
) -> Arc<LiveAgentRegistry> {
    let registry = Arc::new(LiveAgentRegistry::default());
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: resources.clone(),
            adapter,
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(resources.clone()),
            operations: AgentOperationService::new(resources, registry.clone()),
        },
        AgentWorkerConfig::new("ordinary-channel-publication", "fixture-deployment"),
    );
    assert!(worker
        .run_one(agent, std::time::SystemTime::now())
        .await
        .unwrap());
    registry
}
