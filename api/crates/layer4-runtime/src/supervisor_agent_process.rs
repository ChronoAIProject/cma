use layer3_adapter::wire::FencedRequest;
use layer3_adapter::{AgentCloseRequest, AgentLaunch, AgentProcessAction, AgentSlot};

use super::{agent_validation, Supervisor};
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::{Operation, Status, Subject};

impl Supervisor {
    pub(crate) fn agent_process_restart_enabled(&self) -> bool {
        self.config.agent_process_restart_enabled
    }

    pub(super) async fn prove_agent_process_closed(
        &self,
        proof: layer3_adapter::wire::AgentProcessCloseProof,
    ) -> Result<layer3_adapter::wire::AgentProcessCloseProof> {
        let incarnation = agent_validation::close(&proof.close)?;
        let target = &proof.runtime;
        if target.runtime.sandbox_id != self.config.sandbox_id
            || target.runtime.generation != self.config.generation
            || Some(&target.runtime.home_identity) != self.config.home_identity.as_ref()
            || target.boot_id != self.boot_id
        {
            return Err(RuntimeError::rejected(
                "agent_process_close_runtime_changed",
                "the observed runtime changed before Agent Close verification",
            ));
        }
        self.require_credentialed().await?;
        let closed = Operation::close_agent(&proof.close);
        let agent = proof.close.agent_id.as_str();
        let open_id = AgentProcessAction::Open.operation_id(agent, incarnation);
        let opened = self
            .mutations
            .find_process_proof_record(open_id, &closed)
            .await?;
        let exact_open = opened.is_some_and(|record| {
            record.status() == Status::Completed
                && record.operation().request_sha256 == proof.open_request_sha256
                && matches!(record.operation().subject, Subject::OpenAgent { agent_id, cwd, home }
                    if agent_id == agent && cwd == proof.worktree_path
                        && std::path::Path::new(&home) == self.config.agent_home(agent))
        });
        let successor_open = match incarnation.checked_add(1) {
            Some(next) => {
                self.mutations.has_process_writer(agent, next).await?
                    || self
                        .mutations
                        .find_process_proof_record(
                            AgentProcessAction::Open.operation_id(agent, next),
                            &closed,
                        )
                        .await?
                        .is_some()
            }
            None => false,
        };
        let exact_close = self
            .mutations
            .find_process_proof_record(closed.operation_id.clone(), &closed)
            .await?
            .is_some_and(|record| {
                record.status() == Status::Completed && record.operation() == closed
            });
        if !exact_open
            || !exact_close
            || successor_open
            || self.agents.read().await.contains_key(&proof.close.agent_id)
        {
            return Err(RuntimeError::rejected(
                "agent_process_close_unproved",
                "the runtime could not prove this exact Agent process was closed",
            ));
        }
        Ok(proof)
    }

    pub(super) async fn authorize_process_open(
        &self,
        request: &FencedRequest<AgentLaunch>,
    ) -> Result<Operation> {
        let incarnation = agent_validation::open(&request.request)?;
        let operation = Operation::open_agent(&request.request);
        if incarnation == 1 {
            return Ok(operation);
        }
        let writer = request.restart_runtime.as_ref().ok_or_else(|| {
            RuntimeError::rejected(
                "agent_process_restart_runtime_missing",
                "Agent restart requires the observed runtime identity",
            )
        })?;
        if writer.runtime.sandbox_id != self.config.sandbox_id
            || writer.runtime.generation != self.config.generation
            || Some(&writer.runtime.home_identity) != self.config.home_identity.as_ref()
            || writer.boot_id != self.boot_id
        {
            return Err(RuntimeError::rejected(
                "agent_process_restart_runtime_changed",
                "the observed runtime changed before Agent restart",
            ));
        }
        self.require_credentialed().await?;
        let replay = self.mutations.admitted(&operation).await?;
        if !self.config.agent_process_restart_enabled && !replay {
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
        if !replay
            && self
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
