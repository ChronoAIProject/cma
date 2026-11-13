//! Assembled control-plane services and worker handles.
use super::*;
pub(crate) struct ControlPlaneComposition {
    pub document_maintenance: crate::document_artifacts::worker::DocumentMaintenanceWorker,
    pub router: Router,
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
    pub native_prepared: crate::sandbox::native_prepared::NativePreparedWorker,
    pub agent_access_worker: crate::agent_access_worker::AgentAccessWorker,
}
