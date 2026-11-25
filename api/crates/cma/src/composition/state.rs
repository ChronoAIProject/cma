//! Assembled control-plane services and worker handles.
use super::*;
use axum::Router;
mod openapi;

impl ControlPlaneComposition {
    pub(crate) async fn build(
        config: &Config,
        #[cfg(feature = "qualification-hooks")] qualification: Option<cma_qualification::Registry>,
    ) -> Result<Self, String> {
        openapi::warm(layer1_public::rest::openapi_cache::warm).await?;
        Self::assemble(
            config,
            #[cfg(feature = "qualification-hooks")]
            qualification,
        )
        .await
    }
}

pub(crate) struct ControlPlaneComposition {
    pub creation_outcomes: Option<Arc<crate::store::creation_outcomes::CreationOutcomes>>,
    // Reserved for document purge/reclaim and account erasure workers, never routes.
    #[allow(dead_code)]
    pub document_blob_maintenance:
        Option<Arc<dyn layer2_core::document_artifacts::DocumentBlobMaintenance>>,
    pub router: Router,
    pub agent_manager: Option<Arc<crate::agent_manager::AgentManager>>,
    pub agent_relay: Option<Arc<crate::agent_manager::relay::Relay>>,
    pub activity_worker: Option<crate::activity::queue::ActivityWorker>,
    pub workers: crate::worker_supervision::Workers,
    pub worker_observations: crate::store::worker_observations::Observations,
    pub bindings: bindings::BindingComposition,
    pub bots: Option<bots::BotComposition>,
    pub actor_continuations: Option<Arc<crate::bot_setups::ActorContinuationWorker>>,
    pub auth: Arc<AuthState>,
    pub launch_scheduler: crate::trigger::LaunchScheduler,
    pub provenance_backfill: crate::sandbox::AgentProvenanceBackfill,
    pub usage: crate::usage_billing::UsageWorkers,
    pub usage_metrics: crate::sandbox_metrics::MetricsWorkers,
    pub command_scheduler: crate::commands::CommandScheduler,
    pub sandbox_control_scheduler: crate::commands::SandboxControlScheduler,
    pub sandbox_lifecycle: SandboxLifecycleComposition,
    pub native_warm_pools: Arc<crate::warm_pools::NativeWarmPools>,
    pub runtime_pointer: Option<Arc<crate::runtime_pointer::RuntimePointerController>>,
    pub native_prepared: crate::sandbox::native_prepared::NativePreparedWorker,
    pub agent_access_worker: crate::agent_access_worker::AgentAccessWorker,
}
