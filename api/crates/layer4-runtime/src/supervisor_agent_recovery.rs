use layer3_adapter::wire::{AgentHomeReceipt, AgentHomeRemoveReceipt, AgentOpenReceipt};
use layer3_adapter::{AgentCloseRequest, AgentHomeRemoveRequest, AgentSlot};

use super::Supervisor;
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::Subject;
use crate::mutation_store::{Pending, Ticket};

impl Supervisor {
    pub(super) async fn recover_agent_home(&self, pending: Pending) -> Result<()> {
        let Subject::CreateAgentHome { agent_id } = &pending.operation.subject else {
            return Err(corrupt("agent home recovery received another operation"));
        };
        match crate::agent_home::recover_materialization(
            &self.config,
            agent_id,
            &pending.operation.operation_id,
        ) {
            Ok(Some(path)) => {
                pending
                    .ticket
                    .complete(&AgentHomeReceipt {
                        operation_id: pending.operation.operation_id,
                        request_sha256: pending.operation.request_sha256,
                        agent_id: AgentSlot::new(agent_id),
                        path: path.to_string_lossy().into_owned(),
                    })
                    .await
            }
            Ok(None) => Ok(()),
            Err(error) if error.retryable => Err(error),
            Err(error) => {
                tracing::warn!(detail = %error, "agent home recovery failed closed");
                pending.ticket.indeterminate(
                    "agent home creation left ambiguous staged or published state; explicit recovery is required",
                ).await?;
                Ok(())
            }
        }
    }

    pub(super) async fn recover_agent_open(&self, pending: Pending) -> Result<()> {
        let Subject::OpenAgent {
            agent_id,
            cwd,
            home,
        } = &pending.operation.subject
        else {
            return Err(corrupt("agent open recovery received another operation"));
        };
        let agents = self.agents.read().await;
        if let Some(process) = agents.get(&AgentSlot::new(agent_id)) {
            let launch = &process.launch;
            if launch.operation_id != pending.operation.operation_id
                || launch.request_sha256 != pending.operation.request_sha256
                || launch.payload_sha256() != pending.operation.payload_sha256
                || launch.cwd != *cwd
                || launch.home != *home
            {
                return Err(corrupt(
                    "started agent open disagrees with its in-memory bridge",
                ));
            }
            if !process.bridge.is_ended() {
                return pending
                    .ticket
                    .complete(&AgentOpenReceipt {
                        operation_id: pending.operation.operation_id,
                        request_sha256: pending.operation.request_sha256,
                        agent_id: AgentSlot::new(agent_id),
                        cwd: cwd.clone(),
                        home: home.clone(),
                        conversation_id: None,
                    })
                    .await;
            }
        }
        drop(agents);
        pending.ticket.indeterminate(
            "agent open may have spawned before runtime restart; a duplicate process will not be started",
        ).await?;
        Ok(())
    }

    pub(super) async fn recover_agent_close(
        &self,
        authority_fence: u64,
        pending: Pending,
    ) -> Result<()> {
        let Subject::CloseAgent { agent_id } = &pending.operation.subject else {
            return Err(corrupt("agent close recovery received another operation"));
        };
        let request = AgentCloseRequest {
            operation_id: pending.operation.operation_id,
            request_sha256: pending.operation.request_sha256,
            agent_id: AgentSlot::new(agent_id),
        };
        let (ticket, terminal) = pending.ticket.track_terminal();
        terminal.absorb(
            self.execute_close_agent(authority_fence, request, ticket)
                .await,
        )
    }

    pub(super) async fn recover_agent_home_removal(&self, pending: Pending) -> Result<()> {
        let Subject::RemoveAgentHome { agent_id } = &pending.operation.subject else {
            return Err(corrupt(
                "agent home removal recovery received another operation",
            ));
        };
        let request = AgentHomeRemoveRequest {
            operation_id: pending.operation.operation_id,
            request_sha256: pending.operation.request_sha256,
            agent_id: AgentSlot::new(agent_id),
        };
        let result =
            crate::agent_home::remove(&self.config, agent_id).map(|_| AgentHomeRemoveReceipt {
                operation_id: request.operation_id,
                request_sha256: request.request_sha256,
                agent_id: request.agent_id,
            });
        finish_recovery(pending.ticket, result).await
    }
}

async fn finish_recovery<T: serde::Serialize + Sync>(
    ticket: Ticket,
    result: Result<T>,
) -> Result<()> {
    match result {
        Ok(receipt) => ticket.complete(&receipt).await,
        Err(error) if error.retryable => Err(error),
        Err(error) => {
            ticket.fail(&error).await?;
            Ok(())
        }
    }
}

fn corrupt(detail: &str) -> RuntimeError {
    RuntimeError::rejected("mutation_journal_corrupt", detail)
}
