use layer3_adapter::wire::{
    AgentOpenReceipt, AgentProcessCloseProof, AgentRestartFence, FencedRequest,
};
use layer3_adapter::{AgentCloseRequest, AgentLaunch, AgentProcessAction, AgentSlot};

use super::{agent_validation, Supervisor};
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::{Operation, Status, Subject};
use crate::mutation_store::{Begin, Pending};

impl Supervisor {
    pub(crate) fn agent_process_restart_enabled(&self) -> bool {
        self.config.agent_process_restart_enabled
    }

    pub(super) async fn prove_agent_process_closed(
        &self,
        proof: AgentProcessCloseProof,
    ) -> Result<AgentProcessCloseProof> {
        let (incarnation, closed) = self.verify_completed_process_pair(&proof).await?;
        let agent = proof.close.agent_id.as_str();
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
        if successor_open || self.agents.read().await.contains_key(&proof.close.agent_id) {
            return Err(close_unproved());
        }
        Ok(proof)
    }

    async fn verify_completed_process_pair(
        &self,
        proof: &AgentProcessCloseProof,
    ) -> Result<(u64, Operation)> {
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
                && self.matches_open_path(&record.operation(), proof)
        });
        let exact_close = self
            .mutations
            .find_process_proof_record(closed.operation_id.clone(), &closed)
            .await?
            .is_some_and(|record| {
                record.status() == Status::Completed && record.operation() == closed
            });
        if !exact_open || !exact_close {
            return Err(close_unproved());
        }
        Ok((incarnation, closed))
    }

    pub(super) async fn fence_agent_restart(
        &self,
        request: AgentRestartFence,
    ) -> Result<AgentRestartFence> {
        let predecessor = &request.predecessor;
        let incarnation = agent_validation::close(&predecessor.close)?;
        let successor = incarnation
            .checked_add(1)
            .ok_or_else(invalid_restart_fence)?;
        if request.authority_fence == 0
            || request.successor_operation_id
                != AgentProcessAction::Open
                    .operation_id(predecessor.close.agent_id.as_str(), successor)
        {
            return Err(invalid_restart_fence());
        }
        let _writer = self
            .removal_writer_guard()?
            .ok_or_else(invalid_restart_fence)?;
        if let Some(digest) = request.successor_open_request_sha256.as_deref() {
            let close = self
                .prepare_restart_successor_close(&request, successor, digest)
                .await?;
            self.close_agent(request.authority_fence, close).await?;
            return Ok(request);
        }
        // The operation gate excludes physical Open, and both proof reads and
        // authority admission use the same serialized mutation store.
        self.prove_agent_process_closed(predecessor.clone()).await?;
        self.mutations
            .admit(
                request.authority_fence,
                &[Operation::close_agent(&predecessor.close)],
            )
            .await?;
        Ok(request)
    }

    /// The caller holds the operation gate and boot writer guard.
    pub(super) async fn prepare_restart_successor_close(
        &self,
        request: &AgentRestartFence,
        incarnation: u64,
        digest: &str,
    ) -> Result<AgentCloseRequest> {
        let proof = &request.predecessor;
        let (_, predecessor) = self.verify_completed_process_pair(proof).await?;
        let slot = &proof.close.agent_id;
        let agent = slot.as_str();
        // The read also validates saved authority before any Close is admitted.
        if !self
            .mutations
            .has_process_writer(agent, incarnation)
            .await?
        {
            return Err(close_unproved());
        }
        let open = self
            .mutations
            .find_process_proof_record(request.successor_operation_id.clone(), &predecessor)
            .await?
            .ok_or_else(close_unproved)?;
        let operation = open.operation();
        if operation.request_sha256 != digest || !self.matches_open_path(&operation, proof) {
            return Err(close_unproved());
        }
        let close = AgentCloseRequest::new(
            AgentProcessAction::Close.operation_id(agent, incarnation),
            slot.clone(),
        );
        let closing = Operation::close_agent(&close);
        let recorded_close = self
            .mutations
            .find_process_proof_record(closing.operation_id.clone(), &closing)
            .await?;
        let bridge = {
            let agents = self.agents.read().await;
            if let Some(process) = agents.get(slot) {
                if Operation::open_agent(&process.launch) != operation {
                    return Err(close_unproved());
                }
                Some(std::sync::Arc::clone(&process.bridge))
            } else {
                None
            }
        };
        // A durable Open with no observed bridge needs an exact Close journal
        // to explain its absence. Do not open a process to resolve ambiguity.
        if !matches!(open.status(), Status::Started | Status::Completed)
            || (open.status() == Status::Started
                && (recorded_close.is_some()
                    || bridge.as_ref().is_none_or(|bridge| bridge.is_ended())))
            || recorded_close.as_ref().is_some_and(|record| {
                record.operation() != closing
                    || !matches!(record.status(), Status::Started | Status::Completed)
                    || (record.status() == Status::Completed && bridge.is_some())
            })
            || (bridge.is_none() && recorded_close.is_none())
        {
            return Err(close_unproved());
        }
        if let Some(later) = incarnation.checked_add(1) {
            if self.mutations.has_process_writer(agent, later).await?
                || self
                    .mutations
                    .find_process_proof_record(
                        AgentProcessAction::Open.operation_id(agent, later),
                        &closing,
                    )
                    .await?
                    .is_some()
            {
                return Err(close_unproved());
            }
        }
        if open.status() == Status::Started {
            if self
                .mutations
                .recovery_fence(&operation)
                .await?
                .is_none_or(|fence| fence >= request.authority_fence)
            {
                return Err(RuntimeError::rejected(
                    "mutation_authority_stale",
                    "the restart teardown fence must advance beyond the successor Open",
                ));
            }
            // Complete the matching live Open before replacing its writer. A
            // subsequent recovery can then settle Close without superseding Open.
            let Begin::Started(ticket) =
                self.mutations.begin::<AgentOpenReceipt>(&operation).await?
            else {
                return Err(close_unproved());
            };
            self.recover_agent_open(Pending {
                operation: operation.clone(),
                ticket,
            })
            .await?;
            if !self.mutations.completed(&operation).await? {
                return Err(close_unproved());
            }
        }
        self.mutations
            .admit(request.authority_fence, std::slice::from_ref(&closing))
            .await?;
        Ok(close)
    }

    fn matches_open_path(&self, operation: &Operation, proof: &AgentProcessCloseProof) -> bool {
        let agent = proof.close.agent_id.as_str();
        matches!(&operation.subject, Subject::OpenAgent { agent_id, cwd, home }
            if agent_id == agent && cwd == &proof.worktree_path
                && std::path::Path::new(home) == self.config.agent_home(agent))
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

fn invalid_restart_fence() -> RuntimeError {
    RuntimeError::rejected(
        "agent_restart_fence_invalid",
        "the Agent restart fence has invalid successor authority",
    )
}

fn close_unproved() -> RuntimeError {
    RuntimeError::rejected(
        "agent_process_close_unproved",
        "the runtime could not prove this exact Agent process was closed",
    )
}
