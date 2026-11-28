use layer2_core::sandbox::store::{
    apply_sandbox_runtime_loss_with_policy, precheck_runtime_loss_agents,
    precheck_sandbox_runtime_loss, SandboxRuntimeLossApplication, SandboxRuntimeLossOutcome,
    SandboxRuntimeLossRequest, SandboxStoreError,
};
use layer2_core::sandbox::{Agent, AgentLogEntry, Sandbox, SandboxEvent};
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::agent_log_integrity::TransactionLogError;
use super::super::lifecycle_execution::LifecycleLoadError;
use super::super::native_retirement_action::{self, RetirementAction};
use super::super::sandbox_events::{
    append_in_transaction, validate_in_transaction, TransactionEventError,
};
use super::super::sandbox_mongo::{
    record_bson, signed, unsigned, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use super::super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::agents::{load_agents, publish_agents};
use super::telemetry::measure;

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

#[derive(Clone)]
pub(super) struct RecoveryReceipt {
    pub(super) segmented: Vec<crate::store::agent_log_segments::writer_commit::Recorded>,
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
    retirement: Option<&RetirementAction>,
    session: &mut ClientSession,
) -> Result<RecoveryApplication, ApplyError> {
    let document = measure("sandbox_read", async {
        store
            .sandboxes
            .find_one(doc! { "_id": request.sandbox_id.as_str() })
            .session(&mut *session)
            .await
    })
    .await
    .map_err(ApplyError::Driver)?
    .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let current = read_sandbox(&document).map_err(ApplyError::Domain)?;
    validate_retirement(&document, &current, request, retirement)?;
    if store.runtime_loss_bounded_proof_enabled {
        if let Some(outcome) =
            precheck_sandbox_runtime_loss(&current, request).map_err(ApplyError::Domain)?
        {
            return Ok(RecoveryApplication::Complete(outcome));
        }
    }
    let last_seq = document
        .get_i64("last_seq")
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        .and_then(|seq| unsigned(seq, "sandbox event sequence").map_err(SandboxStoreError::Corrupt))
        .map_err(ApplyError::Domain)?;
    let rows = if store.runtime_loss_bounded_proof_enabled {
        let agents = measure(
            "agent_rows",
            super::super::lifecycle_execution::load_sandbox_agents(store, &current.id, session),
        )
        .await?;
        super::telemetry::agent_counts(&agents);
        if let Some(outcome) =
            precheck_runtime_loss_agents(&current, &agents, request).map_err(ApplyError::Domain)?
        {
            return Ok(RecoveryApplication::Complete(outcome));
        }
        super::agents::validate_agents(store, agents, session).await?
    } else {
        load_agents(store, &current.id, session).await?
    };
    let evidence = rows
        .iter()
        .map(|(agent, source)| (agent.clone(), source.through()))
        .collect::<Vec<_>>();
    validate_retirement_children(&current, &evidence, retirement)?;
    let mutation = match apply_sandbox_runtime_loss_with_policy(
        &current,
        &evidence,
        request,
        store.runtime_rotation_recovery_enabled,
    )
    .map_err(ApplyError::Domain)?
    {
        SandboxRuntimeLossApplication::Complete(outcome) => {
            return Ok(RecoveryApplication::Complete(outcome))
        }
        SandboxRuntimeLossApplication::Persist(mutation) => mutation,
    };
    let (agents, logs) = measure(
        "agent_writes",
        publish_agents(store, rows, &mutation.agents, request, session),
    )
    .await?;
    persist_sandbox(
        store, request, retirement, &current, last_seq, *mutation, agents, logs, session,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn persist_sandbox(
    store: &MongoSandboxStore,
    request: &SandboxRuntimeLossRequest,
    retirement: Option<&RetirementAction>,
    current: &Sandbox,
    last_seq: u64,
    mutation: layer2_core::sandbox::store::SandboxRuntimeLossMutation,
    agents: Vec<Agent>,
    logs: Vec<AgentLogEntry>,
    session: &mut ClientSession,
) -> Result<RecoveryApplication, ApplyError> {
    measure(
        "event_validation",
        validate_in_transaction(store, &current.id, last_seq, session),
    )
    .await?;
    measure("sandbox_writes", async {
        let retirement_filter = match retirement {
            Some(action) => record_bson(action)
                .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?,
            None => mongodb::bson::Bson::Document(doc! {"$exists": false}),
        };
        let mut change = crate::store::work_lease_update::replacement(
            sandbox_replacement_fields(&mutation.sandbox).map_err(ApplyError::Domain)?,
        );
        if retirement.is_some() {
            change.push(doc! {"$unset": [native_retirement_action::FIELD, native_retirement_action::OBSERVATION]});
        }
        let advanced = store
            .sandboxes
            .update_one(
                crate::store::work_schema::query(doc! {
                    "_id": current.id.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "channel_reuse_cleanup_execution": { "$exists": false },
                    native_retirement_action::FIELD: retirement_filter,
                    "revision": signed(mutation.expected_revision, "runtime-loss Sandbox revision")
                        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                    "record.runtime": record_bson(&request.expected_runtime)
                        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?,
                    "last_seq": signed(last_seq, "runtime-loss Sandbox event sequence")
                        .map_err(|_| ApplyError::Domain(SandboxStoreError::SequenceConflict))?,
                }),
                change,
            )
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
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
            segmented: Vec::new(),
            sandbox: mutation.sandbox,
            agents,
            logs,
            events,
            outcome: mutation.outcome,
        })))
    })
    .await
}

fn validate_retirement(
    row: &mongodb::bson::Document,
    parent: &Sandbox,
    request: &SandboxRuntimeLossRequest,
    retirement: Option<&RetirementAction>,
) -> Result<(), ApplyError> {
    let current = native_retirement_action::action_from(row).map_err(ApplyError::Domain)?;
    if current.as_ref() != retirement
        || retirement
            .is_some_and(|action| !request.native_retirement_finality || !action.matches(parent))
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}

fn validate_retirement_children(
    parent: &Sandbox,
    evidence: &[(Agent, u64)],
    retirement: Option<&RetirementAction>,
) -> Result<(), ApplyError> {
    if let Some(action) = retirement {
        let agents = evidence
            .iter()
            .map(|(agent, _)| agent.clone())
            .collect::<Vec<_>>();
        if !action.covers(parent, &agents).map_err(ApplyError::Domain)? {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(())
}
