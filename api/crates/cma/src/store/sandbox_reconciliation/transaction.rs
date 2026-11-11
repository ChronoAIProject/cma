use layer2_core::sandbox::store::{
    apply_sandbox_runtime_loss, SandboxRuntimeLossApplication, SandboxRuntimeLossOutcome,
    SandboxRuntimeLossRequest, SandboxStoreError,
};
use layer2_core::sandbox::{Agent, AgentLogEntry, Sandbox, SandboxEvent};
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::agent_log_integrity::TransactionLogError;
use super::super::lifecycle_execution::LifecycleLoadError;
use super::super::sandbox_events::{
    append_in_transaction, validate_in_transaction, TransactionEventError,
};
use super::super::sandbox_mongo::{
    record_bson, signed, unsigned, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use super::super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::agents::{load_agents, publish_agents};

mod scoped;
pub(super) use scoped::apply_scoped;

#[derive(Debug)]
pub(super) enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<LifecycleLoadError> for ApplyError {
    fn from(error: LifecycleLoadError) -> Self {
        match error {
            LifecycleLoadError::Domain(error) => Self::Domain(error),
            LifecycleLoadError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<TransactionEventError> for ApplyError {
    fn from(error: TransactionEventError) -> Self {
        match error {
            TransactionEventError::Domain(error) => Self::Domain(error),
            TransactionEventError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<TransactionLogError> for ApplyError {
    fn from(error: TransactionLogError) -> Self {
        match error {
            TransactionLogError::Domain(error) => Self::Domain(error),
            TransactionLogError::Driver(error) => Self::Driver(error),
        }
    }
}

pub(super) struct RecoveryReceipt {
    pub(super) sandbox: Sandbox,
    pub(super) agents: Vec<Agent>,
    pub(super) logs: Vec<AgentLogEntry>,
    pub(super) events: Vec<SandboxEvent>,
    pub(super) outcome: SandboxRuntimeLossOutcome,
}

pub(super) enum RecoveryApplication {
    Persist(Box<RecoveryReceipt>),
    Complete(SandboxRuntimeLossOutcome),
}

pub(super) async fn apply(
    store: &MongoSandboxStore,
    request: &SandboxRuntimeLossRequest,
    session: &mut ClientSession,
) -> Result<RecoveryApplication, ApplyError> {
    let document = store
        .sandboxes
        .find_one(doc! { "_id": request.sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let current = read_sandbox(&document).map_err(ApplyError::Domain)?;
    let last_seq = document
        .get_i64("last_seq")
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        .and_then(|seq| unsigned(seq, "sandbox event sequence").map_err(SandboxStoreError::Corrupt))
        .map_err(ApplyError::Domain)?;
    let rows = load_agents(store, &current.id, session).await?;
    let evidence = rows
        .iter()
        .map(|(agent, source)| (agent.clone(), source.through()))
        .collect::<Vec<_>>();
    let mutation = match apply_sandbox_runtime_loss(&current, &evidence, request)
        .map_err(ApplyError::Domain)?
    {
        SandboxRuntimeLossApplication::Complete(outcome) => {
            return Ok(RecoveryApplication::Complete(outcome))
        }
        SandboxRuntimeLossApplication::Persist(mutation) => mutation,
    };
    let (agents, logs) = publish_agents(store, rows, &mutation.agents, request, session).await?;
    persist_sandbox(
        store, request, &current, last_seq, mutation, agents, logs, session,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn persist_sandbox(
    store: &MongoSandboxStore,
    request: &SandboxRuntimeLossRequest,
    current: &Sandbox,
    last_seq: u64,
    mutation: layer2_core::sandbox::store::SandboxRuntimeLossMutation,
    agents: Vec<Agent>,
    logs: Vec<AgentLogEntry>,
    session: &mut ClientSession,
) -> Result<RecoveryApplication, ApplyError> {
    validate_in_transaction(store, &current.id, last_seq, session).await?;
    let advanced = store.sandboxes.update_one(
        doc! {
            "_id": current.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "channel_reuse_cleanup_execution": { "$exists": false },
            "revision": signed(mutation.expected_revision, "runtime-loss Sandbox revision")
                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
            "record.runtime": record_bson(&request.expected_runtime)
                .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?,
            "last_seq": signed(last_seq, "runtime-loss Sandbox event sequence")
                .map_err(|_| ApplyError::Domain(SandboxStoreError::SequenceConflict))?,
        },
        doc! { "$set": sandbox_replacement_fields(&mutation.sandbox).map_err(ApplyError::Domain)? },
    ).session(&mut *session).await.map_err(ApplyError::Driver)?;
    if advanced.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    crate::store::sandbox_usage::save::record_saved(
        store,
        session,
        &mutation.sandbox,
        current.revision,
    )
    .await?;
    let mut events = Vec::with_capacity(mutation.events.len());
    for kind in mutation.events {
        events.push(
            append_in_transaction(
                store,
                session,
                &current.id,
                request.observed_at.clone(),
                kind,
            )
            .await?,
        );
    }
    Ok(RecoveryApplication::Persist(Box::new(RecoveryReceipt {
        sandbox: mutation.sandbox,
        agents,
        logs,
        events,
        outcome: mutation.outcome,
    })))
}
