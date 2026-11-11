//! Shared restart/loss routing has no ordinary-reconnect fallback for v2.
use super::{recovery::RestrictedRecoveryRuntime, AgentWorker};
use crate::store::agent_capacity_recovery::{RecoveryService, RestartDecision};
use layer2_core::{
    sandbox::agent_capacity_recovery::{
        ports::{CapacityRecoveryRuntime, CommitOutcome, RecoveryError},
        state::RecoveryPhase,
    },
    AgentId,
};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReconnectAction {
    Ordinary,
    Recovered,
    Deferred,
}

#[async_trait::async_trait]
pub(crate) trait CapacityRecoveryReconnect: Send + Sync {
    async fn route(&self, id: &AgentId) -> Result<ReconnectAction, RecoveryError>;
}

pub(crate) struct RecoveryReconnect<R> {
    service: Arc<RecoveryService>,
    runtime: R,
    holder: String,
}

impl<R: CapacityRecoveryRuntime> RecoveryReconnect<R> {
    pub(crate) fn new(service: Arc<RecoveryService>, runtime: R, holder: String) -> Self {
        Self {
            service,
            runtime,
            holder,
        }
    }
}

#[async_trait::async_trait]
impl<R: CapacityRecoveryRuntime> CapacityRecoveryReconnect for RecoveryReconnect<R> {
    async fn route(&self, id: &AgentId) -> Result<ReconnectAction, RecoveryError> {
        match self.service.restart_decision(id).await? {
            RestartDecision::Legacy => Ok(ReconnectAction::Ordinary),
            RestartDecision::Deferred => Ok(ReconnectAction::Deferred),
            RestartDecision::Completed(request) => {
                // The route decision grants no authority. This entry obtains
                // fresh accepted-work authority before and after native IO.
                Ok(
                    match self
                        .service
                        .run_accepted(&self.runtime, &request, &self.holder)
                        .await?
                    {
                        CommitOutcome::Recorded(work) if work.phase == RecoveryPhase::Completed => {
                            ReconnectAction::Recovered
                        }
                        _ => ReconnectAction::Deferred,
                    },
                )
            }
        }
    }
}

impl AgentWorker {
    pub(super) fn install_capacity_restart(&self, service: Arc<RecoveryService>) {
        let runtime = RestrictedRecoveryRuntime::new(
            self.dependencies.adapter.clone(),
            self.dependencies.registry.clone(),
            self.dependencies.updates.clone(),
        );
        self.capacity_restart
            .set(Arc::new(RecoveryReconnect::new(
                service,
                runtime,
                self.config.holder.clone(),
            )))
            .unwrap_or_else(|_| panic!("capacity recovery restart is assembled once"));
    }
}
