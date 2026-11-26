//! Owner management projection over the execution store. Only the transactional
//! finalizer can create an effective receipt; observation never drives effects.

use super::{execution::*, *};
use layer3_adapter::resource_execution_protocol as wire;
use std::{sync::Arc, time::Duration};

const STATUS_READ_BUDGET: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationStage {
    Pending,
    EvidenceReceived,
    Effective,
}

#[derive(Clone)]
pub struct RevocationStatus {
    pub operation_id: String,
    pub selector: ResourceSelector,
    pub share_id: String,
    pub expected_revision: u64,
    pub stage: RevocationStage,
    pub participants: u32,
    pub stopped: u32,
    pub unresolved_effects: u32,
    pub deadline_unix_ms: u64,
    pub deadline_exceeded: bool,
    pub receipt: Option<ShareMutationReceipt>,
}

#[async_trait::async_trait]
pub trait ResourceRevocations: Send + Sync {
    async fn request(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        command: ShareCommand,
    ) -> ExecutionResult<RevocationStatus>;
    async fn status(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        selector: &ResourceSelector,
        operation: &str,
    ) -> ExecutionResult<RevocationStatus>;
}

pub struct ResourceRevocationService {
    coordinator: ResourceExecutionCoordinator,
    store: Arc<dyn ResourceExecutionStore>,
}

impl ResourceRevocationService {
    /// Composition supplies an already activated store. This never initializes
    /// a latch, enables admissions, or qualifies any participant implementation.
    pub async fn new(
        access: ResourceAccessService,
        store: Arc<dyn ResourceExecutionStore>,
    ) -> ExecutionResult<Self> {
        let policy = ResourceExecutionPolicy::enforce(store.clone(), false).await?;
        Ok(Self {
            coordinator: ResourceExecutionCoordinator::new(access, policy),
            store,
        })
    }

    async fn project(
        &self,
        operation: &RevocationOperationV1,
    ) -> ExecutionResult<RevocationStatus> {
        operation.validate()?;
        let mut after = 0;
        let mut stopped = 0;
        let mut unresolved_effects = 0;
        while after < operation.frozen_members {
            let page = self
                .store
                .frozen_members(&operation.key.operation_id, after, MAX_PAGE_MEMBERS)
                .await?;
            if page.is_empty() || page.len() > MAX_PAGE_MEMBERS as usize {
                return Err(ExecutionError::Corrupt);
            }
            for member in page {
                if member.ordinal != after + 1 || member.epoch != operation.epoch {
                    return Err(ExecutionError::EvidenceMismatch);
                }
                let request = member.barrier_request(operation)?;
                if let Some(barrier) = &member.barrier {
                    barrier.validate_for(&request)?;
                }
                stopped += u32::from(member.completion.is_some());
                unresolved_effects += self.unresolved(&member).await?;
                after = member.ordinal;
            }
        }
        let (stage, receipt) = match &operation.phase {
            RevocationPhaseV1::Effective { receipt } => {
                if stopped != operation.frozen_members {
                    return Err(ExecutionError::EvidenceMismatch);
                }
                (RevocationStage::Effective, Some(*receipt.clone()))
            }
            RevocationPhaseV1::Pending if stopped == operation.frozen_members => {
                (RevocationStage::EvidenceReceived, None)
            }
            RevocationPhaseV1::Pending => (RevocationStage::Pending, None),
        };
        Ok(RevocationStatus {
            operation_id: operation.key.operation_id.as_str().into(),
            selector: operation.intent.selector.clone(),
            share_id: operation.key.family.share_id.as_str().into(),
            expected_revision: operation.intent.expected_share_revision,
            stage,
            participants: operation.frozen_members,
            stopped,
            unresolved_effects,
            deadline_unix_ms: operation.deadline_unix_ms,
            deadline_exceeded: operation.deadline_exceeded_at(unix_millis_now()?)?,
            receipt,
        })
    }

    async fn unresolved(&self, member: &ExecutionRecordV1) -> ExecutionResult<u32> {
        let mut after = 0;
        let mut unresolved = 0;
        while after < member.effect_count {
            let page = self
                .store
                .effects_for(&member.registration, after, MAX_PAGE_MEMBERS)
                .await?;
            if page.is_empty() || page.len() > MAX_PAGE_MEMBERS as usize {
                return Err(ExecutionError::Corrupt);
            }
            for effect in page {
                effect.validate()?;
                let intent = &effect.permission.intent;
                if intent.registration != member.registration
                    || intent.grant != member.basis
                    || (wire::MemberKey::Execution {
                        execution: intent.execution.clone(),
                    }) != member.member
                    || effect.permission.ledger_sequence.get() != u64::from(after) + 1
                    || after >= member.effect_count
                {
                    return Err(ExecutionError::EvidenceMismatch);
                }
                unresolved += u32::from(!effect.observations.last().is_some_and(|observation| {
                    matches!(
                        observation.outcome,
                        wire::EffectOutcome::CommitConfirmed | wire::EffectOutcome::Rejected
                    )
                }));
                after += 1;
            }
        }
        Ok(unresolved)
    }
}

#[async_trait::async_trait]
impl ResourceRevocations for ResourceRevocationService {
    async fn request(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        command: ShareCommand,
    ) -> ExecutionResult<RevocationStatus> {
        let operation = self
            .coordinator
            .begin_revocation(context, credential, command)
            .await?;
        self.status(
            context,
            credential,
            &operation.intent.selector,
            operation.key.operation_id.as_str(),
        )
        .await
    }

    async fn status(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        selector: &ResourceSelector,
        operation: &str,
    ) -> ExecutionResult<RevocationStatus> {
        let id = wire::Digest::new(operation).map_err(|_| ExecutionError::AuthorityChanged)?;
        tokio::time::timeout(STATUS_READ_BUDGET, async {
            let operation = self
                .coordinator
                .revocation_status(context, credential, selector, &id)
                .await?;
            if operation.key.operation_id != id {
                return Err(ExecutionError::EvidenceMismatch);
            }
            let projection = self.project(&operation).await?;
            // Evidence enumeration can wait on storage. Recheck owner/credential and
            // incarnation before exposing the already-authorized bounded snapshot.
            let current = self
                .coordinator
                .revocation_status(context, credential, selector, &id)
                .await?;
            if current.intent != operation.intent
                || current.key != operation.key
                || current.epoch != operation.epoch
                || current.frozen_members != operation.frozen_members
                || current.frozen_at_unix_ms != operation.frozen_at_unix_ms
                || current.deadline_unix_ms != operation.deadline_unix_ms
            {
                return Err(ExecutionError::EvidenceMismatch);
            }
            Ok(projection)
        })
        .await
        .map_err(|_| ExecutionError::Unavailable)?
    }
}

#[cfg(test)]
mod tests;
