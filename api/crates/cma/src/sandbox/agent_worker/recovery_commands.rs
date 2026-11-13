//! Actual authorized command implementation for S2's protocol composition.
use std::sync::Arc;

use layer2_core::{
    sandbox::agent_capacity_recovery::{
        commands::{CapacityRecoveryCommands, RecoveryStatus},
        ports::{CommitOutcome, RecoveryError},
        state::{RecoveryRequest, RecoveryWork},
    },
    AgentId, Caller,
};

use super::AgentWorker;
use crate::store::agent_capacity_recovery::RecoveryService;

pub(crate) struct AuthorizedRecoveryCommands {
    service: Arc<RecoveryService>,
    worker: Arc<AgentWorker>,
}

impl AuthorizedRecoveryCommands {
    pub(crate) fn new(service: Arc<RecoveryService>, worker: Arc<AgentWorker>) -> Self {
        Self { service, worker }
    }
}

#[async_trait::async_trait]
impl CapacityRecoveryCommands for AuthorizedRecoveryCommands {
    async fn status(
        &self,
        caller: &Caller,
        agent_id: &AgentId,
        request: Option<&RecoveryRequest>,
    ) -> Result<RecoveryStatus, RecoveryError> {
        self.service.status(caller, agent_id, request).await
    }

    async fn admit(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.service.admit(caller, request).await
    }

    async fn resume(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.worker
            .run_capacity_recovery(&self.service, caller, request)
            .await
    }
}
