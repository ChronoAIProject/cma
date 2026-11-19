use super::*;
use crate::{
    agent_manager::{relay::Relay, AgentManager},
    store::agent_observation::MongoAgentObservation,
};
use layer2_core::sandbox::agent_observation::AgentObservationService;

pub(super) struct Observation(Option<(Arc<MongoAgentObservation>, Arc<AgentObservationService>)>);

pub(super) async fn prepare(
    config: &Config,
    stores: &MongoStores,
    manager: &Option<Arc<AgentManager>>,
    projections: AgentProjectionServices,
) -> Result<(AgentProjectionServices, Observation), String> {
    if !config.features.agent_shared_observation {
        return Ok((projections, Observation(None)));
    }
    let manager = manager
        .as_ref()
        .ok_or("shared observation requires Agent Manager replicas")?;
    let store =
        MongoAgentObservation::open(stores.sandbox_model.clone(), manager.coordinator.clone())
            .await
            .map_err(|error| error.to_string())?;
    let service = AgentObservationService::new(projections.reads(), store.clone());
    Ok((
        projections.with_observation(service.clone()),
        Observation(Some((store, service))),
    ))
}

impl Observation {
    pub(super) fn finish(
        self,
        relay: &Option<Arc<Relay>>,
        lifecycle: &SandboxLifecycleComposition,
        projections: &AgentProjectionServices,
    ) -> Result<(), String> {
        if let Some((store, service)) = self.0 {
            service
                .install_hints(lifecycle.live_updates())
                .map_err(|error| error.to_string())?;
            relay
                .as_ref()
                .ok_or("shared observation requires the peer relay")?
                .install_observation(store, projections.reads(), projections.command_service())?;
        }
        Ok(())
    }
}
