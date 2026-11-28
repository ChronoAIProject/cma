//! Assembled control-plane services and worker handles.
use super::*;
use axum::Router;
mod openapi;

pub(crate) enum StartupComposition {
    Execution(Box<ControlPlaneComposition>),
    ReleaseQuarantined(Router),
}

impl StartupComposition {
    pub(crate) async fn build(
        config: &Config,
        #[cfg(feature = "qualification-hooks")] qualification: Option<cma_qualification::Registry>,
    ) -> Result<Self, String> {
        openapi::warm(layer1_public::rest::openapi_cache::warm).await?;
        let stores = stores::connect(config).await?;
        if stores
            .runtime_pointer
            .as_ref()
            .is_some_and(|source| source.is_quarantined())
        {
            return runtime_quarantine::compose(config, stores)
                .await
                .map(Self::ReleaseQuarantined);
        }
        Ok(Self::Execution(Box::new(
            ControlPlaneComposition::assemble(
                config,
                stores,
                #[cfg(feature = "qualification-hooks")]
                qualification,
            )
            .await?,
        )))
    }
}

pub(crate) struct ControlPlaneComposition {
    pub creation_outcomes: Option<Arc<crate::store::creation_outcomes::CreationOutcomes>>,
    pub document_reclaimer: Option<crate::document_artifacts::ReclamationWorker>,
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
