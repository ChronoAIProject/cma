//! First adoption belongs to the original lifecycle publication and live lease.
use super::*;
use crate::store::agent_capacity_recovery::observations::{cohort, Outcome, Stage};
use crate::store::agent_log_integrity::AppendCapacity;
use crate::store::agent_log_segments::{
    capture, incarnation,
    publication::{self, PublicationIdentity},
    valid_digest,
    writer_admission::{Admission, AdmissionKind, PENDING_WORKER_PREFIX},
    writer_bootstrap::BootstrapPlan,
    writer_gate, writer_plan, writer_reconcile,
};
use layer2_core::sandbox::{
    agent_capacity_recovery::{CapacityRecoveryContext, CapacityRecoveryDisposition},
    AgentLogKind, Workspace,
};
use mongodb::bson::Binary;

mod lineage;
mod staging;
#[cfg(test)]
pub(crate) mod test_hooks;
pub(in crate::store) mod witness;
use witness::{State, Witness};

pub(super) struct Source {
    current: Agent,
    witness: Witness,
    fresh: bool,
}

pub(super) struct Prepared {
    witness: Witness,
    pub(super) admission: Admission,
    pub(super) bootstrap: BootstrapPlan,
}

pub(super) use staging::prepare;

impl Prepared {
    pub(super) fn operation(&self) -> PublicationIdentity {
        self.witness.operation()
    }

    pub(super) fn fence_fields(
        &self,
        filter: &mut Document,
        fields: &mut Document,
    ) -> Result<(), ApplyError> {
        filter.insert(witness::FIELD, self.witness.document()?);
        let mut published = self.witness.clone();
        published.state = State::Published;
        fields.insert(witness::FIELD, published.document()?);
        Ok(())
    }
}

fn is_publication(mutation: &AgentWorkMutation) -> bool {
    matches!(
        mutation.agent_logs.as_slice(),
        [
            AgentLogKind::ConversationOpened { .. },
            AgentLogKind::StatusChanged {
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }
        ] | [AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::PublishConnection
        }]
    )
}

fn identity(
    mutation: &AgentWorkMutation,
    lineage: &Binary,
) -> Result<PublicationIdentity, ApplyError> {
    Ok(PublicationIdentity {
        key: format!(
            "{PENDING_WORKER_PREFIX}{}:{}",
            mutation.expected_agent_revision, mutation.fence
        ),
        fingerprint: publication::request_digest(&(
            PENDING_WORKER_PREFIX,
            &mutation.agent,
            &mutation.agent_logs,
            &mutation.sandbox_event,
            mutation
                .worktree
                .as_ref()
                .map(|v| (v.expected_revision, &v.worktree)),
            mutation.expected_agent_revision,
            &mutation.holder,
            mutation.fence,
            &mutation.appended_at,
            lineage,
        ))
        .map_err(ApplyError::Domain)?,
        appended_at: mutation.appended_at.clone(),
    })
}

pub(super) async fn select(
    store: &MongoSandboxStore,
    row: &Document,
    current: &Agent,
    mutation: &AgentWorkMutation,
    segmented: bool,
    observed: Option<AgentWorkPublicationParents>,
    scope: &mut TransactionScope<'_>,
) -> Result<(), ApplyError> {
    let publication = is_publication(mutation);
    let pending_witness = witness::pending_step(current) && row.contains_key(witness::FIELD);
    if !publication && !pending_witness {
        return Ok(());
    }
    let persisted = Witness::read(row)?;
    if let Some(witness) = persisted {
        if witness.expected_revision == mutation.expected_agent_revision {
            let operation = identity(mutation, &witness.lineage)?;
            witness.require_operation(&operation)?;
            if witness.state == State::Published {
                let logs = witness.logs(mutation)?;
                let result =
                    publication::result_digest(&mutation.agent).map_err(ApplyError::Domain)?;
                match writer_reconcile::reconcile_scoped(
                    store,
                    &operation,
                    &mutation.agent,
                    &logs,
                    &result,
                    scope,
                )
                .await
                .map_err(ApplyError::Domain)?
                {
                    writer_reconcile::Outcome::Recorded(_) => {
                        return Err(ApplyError::PendingReplayed)
                    }
                    writer_reconcile::Outcome::Unknown => return Err(witness::unresolved()),
                }
            }
            if witness.state == State::Admitted && !segmented {
                witness.require_source(current, mutation)?;
                return Err(ApplyError::StagePending(Box::new(Source {
                    current: current.clone(),
                    witness,
                    fresh: false,
                })));
            }
            return Err(witness::unresolved());
        }
        // A superseding lifecycle intent will use its own ordinary cleanup paths.
        if witness.state != State::Published && witness::pending_step(current) {
            return Err(witness::unresolved());
        }
    }
    if !publication {
        return Ok(());
    }
    let Some(observed) = observed
        .filter(|_| !segmented && store.segment_adoption_enabled && witness::pending_step(current))
    else {
        return Ok(());
    };
    let database = store.client.database(&store.agents.namespace().db);
    if !writer_gate::selects_pending_worker_scoped(&database, current, scope).await? {
        return Ok(());
    }
    cohort(Stage::Scanned, Outcome::Observed);
    match select_source(store, current, mutation, observed, scope).await {
        Ok(Some(witness)) => {
            cohort(Stage::Eligible, Outcome::Observed);
            Err(ApplyError::StagePending(Box::new(Source {
                current: current.clone(),
                witness,
                fresh: true,
            })))
        }
        Ok(None) => {
            cohort(Stage::Skipped, Outcome::Observed);
            Ok(())
        }
        Err(error) => {
            cohort(Stage::Blocked, Outcome::Refused);
            Err(error)
        }
    }
}

async fn select_source(
    store: &MongoSandboxStore,
    current: &Agent,
    mutation: &AgentWorkMutation,
    observed: AgentWorkPublicationParents,
    scope: &mut TransactionScope<'_>,
) -> Result<Option<Witness>, ApplyError> {
    let (_, logs) = final_agent_and_logs(current, mutation)?;
    if publication_capacity(current, mutation) != AppendCapacity::Normal {
        return Ok(None);
    }
    let captured = capture::capture_transaction(store, current, scope)
        .await
        .map_err(|error| match error {
            TransactionLogError::Domain(SandboxStoreError::AgentLogCapacityExceeded) => {
                witness::unresolved()
            }
            other => other.into(),
        })?;
    if !writer_plan::needs_normal_rollover(captured.active_head(), &logs)
        .map_err(ApplyError::Domain)?
    {
        return Ok(None);
    }
    let lineage = lineage::read(store, current, scope).await?;
    if lineage.revisions() != observed {
        return Err(witness::unresolved());
    }
    let now = layer2_core::now_rfc3339();
    lineage.require(current, mutation, &now)?;
    let proof = lineage.proof().map_err(ApplyError::Domain)?;
    let operation = identity(mutation, &proof)?;
    Ok(Some(Witness::new(current, mutation, proof, operation)?))
}

pub(super) async fn capture_prepared(
    store: &MongoSandboxStore,
    row: &Document,
    current: &Agent,
    mutation: &AgentWorkMutation,
    prepared: &Prepared,
    scope: &mut TransactionScope<'_>,
) -> Result<capture::CapturedLog, ApplyError> {
    let witness = &prepared.witness;
    if Witness::read(row)?.as_ref() != Some(witness) {
        return Err(witness::unresolved());
    }
    witness.require_source(current, mutation)?;
    let parents = lineage::read(store, current, scope).await?;
    parents.require(current, mutation, &layer2_core::now_rfc3339())?;
    if parents.proof().map_err(ApplyError::Domain)? != witness.lineage {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    parents.fence(store, current, scope).await?;
    Ok(capture::capture_transaction(store, current, scope).await?)
}

impl PublicationReceipt {
    pub(in crate::store) fn pending_adoption(&self) -> bool {
        self.segmented
            .as_ref()
            .is_some_and(|receipt| receipt.operation.key.starts_with(PENDING_WORKER_PREFIX))
    }
}
