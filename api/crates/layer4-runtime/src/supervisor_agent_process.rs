use layer3_adapter::wire::FencedRequest;
use layer3_adapter::{AgentCloseRequest, AgentLaunch, AgentProcessAction, AgentSlot};

use super::{agent_validation, Supervisor};
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::{Operation, Status, Subject};

impl Supervisor {
    pub(super) async fn authorize_process_open(
        &self,
        request: &FencedRequest<AgentLaunch>,
    ) -> Result<Operation> {
        let incarnation = agent_validation::open(&request.request)?;
        let operation = Operation::open_agent(&request.request);
        if incarnation == 1 || self.mutations.admitted(&operation).await? {
            return Ok(operation);
        }
        if !self.config.agent_process_restart_enabled {
            return Err(RuntimeError::rejected(
                "agent_process_restart_disabled",
                "Agent process restart is disabled",
            ));
        }
        let agent = request.request.agent_id.as_str();
        let predecessor = Operation::close_agent(&AgentCloseRequest::new(
            AgentProcessAction::Close.operation_id(agent, incarnation - 1),
            AgentSlot::new(agent),
        ));
        let previous_open = AgentProcessAction::Open.operation_id(agent, incarnation - 1);
        if !self.mutations.completed(&predecessor).await?
            || !self
                .mutations
                .find_operation(previous_open, &operation)
                .await?
                .is_some_and(|record| {
                    record.status() == Status::Completed
                        && record.operation().subject == operation.subject
                })
        {
            return Err(RuntimeError::rejected(
                "agent_process_predecessor_unclosed",
                "the previous Agent process has no completed open and close on this home and worktree",
            ));
        }
        if self
            .mutations
            .recovery_fence(&predecessor)
            .await?
            .is_none_or(|fence| fence >= request.authority_fence)
        {
            return Err(RuntimeError::rejected(
                "mutation_authority_stale",
                "the new Agent process fence must advance beyond the previous close",
            ));
        }
        Ok(operation)
    }

    pub(super) async fn authorize_process_close(
        &self,
        request: &AgentCloseRequest,
    ) -> Result<Operation> {
        let incarnation = agent_validation::close(request)?;
        let operation = Operation::close_agent(request);
        if self.mutations.completed(&operation).await? {
            return Ok(operation);
        }
        self.close_process_bridge(request).await?;
        if incarnation > 1 {
            let agent = request.agent_id.as_str();
            let open_id = AgentProcessAction::Open.operation_id(agent, incarnation);
            let opened = self.mutations.find_operation(open_id, &operation).await?;
            if !opened.is_some_and(|record| {
                matches!(record.operation().subject, Subject::OpenAgent { agent_id, .. } if agent_id == agent)
            }) {
                return Err(RuntimeError::rejected(
                    "agent_process_incarnation_unknown",
                    "no Open was recorded for this Agent process",
                ));
            }
        }
        Ok(operation)
    }
}
