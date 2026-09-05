use layer3_adapter::wire::{AgentHomeReceipt, AgentHomeRemoveReceipt, AgentOpenReceipt};
use layer3_adapter::{AgentCloseRequest, AgentHomeRemoveRequest, AgentSlot};

use super::Supervisor;
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::{Pending, Subject, Ticket};

impl Supervisor {
    pub(super) fn recover_agent_home(&self, pending: Pending) -> Result<()> {
        let Subject::CreateAgentHome { agent_id } = &pending.operation.subject else {
            return Err(corrupt("agent home recovery received another operation"));
        };
        match crate::agent_home::recover_materialization(
            &self.config,
            agent_id,
            &pending.operation.operation_id,
        ) {
            Ok(Some(path)) => pending.ticket.complete(&AgentHomeReceipt {
                operation_id: pending.operation.operation_id,
                request_sha256: pending.operation.request_sha256,
                agent_id: AgentSlot::new(agent_id),
                path: path.to_string_lossy().into_owned(),
            }),
            Ok(None) => Ok(()),
            Err(error) if error.retryable => Err(error),
            Err(error) => {
                tracing::warn!(detail = %error, "agent home recovery failed closed");
                pending.ticket.indeterminate(
                    "agent home creation left ambiguous staged or published state; explicit recovery is required",
                )?;
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
                return pending.ticket.complete(&AgentOpenReceipt {
                    operation_id: pending.operation.operation_id,
                    request_sha256: pending.operation.request_sha256,
                    agent_id: AgentSlot::new(agent_id),
                    cwd: cwd.clone(),
                    home: home.clone(),
                    conversation_id: None,
                });
            }
        }
        drop(agents);
        pending.ticket.indeterminate(
            "agent open may have spawned before runtime restart; a duplicate process will not be started",
        )?;
        Ok(())
    }

    pub(super) async fn recover_agent_close(&self, pending: Pending) -> Result<()> {
        let Subject::CloseAgent { agent_id } = &pending.operation.subject else {
            return Err(corrupt("agent close recovery received another operation"));
        };
        if !self
            .agents
            .read()
            .await
            .contains_key(&AgentSlot::new(agent_id))
        {
            pending.ticket.indeterminate(
                "agent close lost its process handle before the termination outcome became durable",
            )?;
            return Ok(());
        }
        let request = AgentCloseRequest {
            operation_id: pending.operation.operation_id,
            request_sha256: pending.operation.request_sha256,
            agent_id: AgentSlot::new(agent_id),
        };
        absorb(self.execute_close_agent(request, pending.ticket).await)
    }

    pub(super) fn recover_agent_home_removal(&self, pending: Pending) -> Result<()> {
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
        finish_recovery(pending.ticket, result)
    }
}

fn finish_recovery<T: serde::Serialize>(ticket: Ticket, result: Result<T>) -> Result<()> {
    match result {
        Ok(receipt) => ticket.complete(&receipt),
        Err(error) if error.retryable => Err(error),
        Err(error) => {
            ticket.fail(&error)?;
            Ok(())
        }
    }
}

fn absorb(result: Result<layer3_adapter::wire::ResponseFrame>) -> Result<()> {
    match result {
        Ok(_) => Ok(()),
        Err(error) if !error.retryable => Ok(()),
        Err(error) => Err(error),
    }
}

fn corrupt(detail: &str) -> RuntimeError {
    RuntimeError::rejected("mutation_journal_corrupt", detail)
}
