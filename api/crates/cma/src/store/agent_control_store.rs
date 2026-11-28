//! Atomic Mongo scheduling for owner-authorized agent stop and deletion.

use layer2_core::sandbox::store::{
    AgentControlResult, AgentControlStore, AgentControlStoreError, AgentDeleteRequest,
    AgentStopRequest,
};
use layer2_core::sandbox::Agent;

use super::agent_control_transaction::{schedule_delete, schedule_stop};
use super::sandbox_mongo::MongoSandboxStore;

#[async_trait::async_trait]
impl AgentControlStore for MongoSandboxStore {
    async fn begin_agent_worktree_recovery(
        &self,
        id: &layer2_core::AgentId,
        caller: &layer2_core::Caller,
        operation_id: &str,
        successor_id: &layer2_core::WorktreeId,
        now: String,
    ) -> Result<Agent, AgentControlStoreError> {
        super::agent_worktree_recovery::begin(self, id, caller, operation_id, successor_id, now)
            .await
    }

    async fn schedule_agent_restart(
        &self,
        request: layer2_core::sandbox::store::AgentRestartRequest,
    ) -> Result<layer2_core::sandbox::store::AgentRestartAcceptance, AgentControlStoreError> {
        super::agent_control_transaction::schedule_restart(self, request).await
    }

    async fn schedule_agent_stop(
        &self,
        request: AgentStopRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        schedule_stop(self, request).await
    }

    async fn schedule_agent_delete(
        &self,
        request: AgentDeleteRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        schedule_delete(self, request).await
    }
}

pub(super) use cma_am_agent_commands::control::{
    is_delete_work, is_stop_work, plan_delete, plan_stop,
};
