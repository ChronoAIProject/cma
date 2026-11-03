use futures_util::TryStreamExt;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    apply_unpublished_runtime_recovery, AgentLogKind, Sandbox, SandboxEvent,
    SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoverySnapshot,
};
use layer2_core::SandboxId;
use mongodb::bson::{doc, Timestamp};
use mongodb::options::ReadConcern;
use mongodb::ClientSession;

use super::super::agent_log::scan_complete_log_with_session;
use super::super::agent_log_integrity::TransactionLogError;
use super::super::lifecycle_execution::{load_sandbox_agents, LifecycleLoadError};
use super::super::sandbox_events::{
    append_in_transaction, reconcile_event_with_snapshot_session, validate_in_transaction,
    TransactionEventError,
};
use super::super::sandbox_mongo::{signed, unsigned, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::super::workspaces::read_workspace;

pub(super) enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl ApplyError {
    pub(super) fn domain(self) -> SandboxStoreError {
        match self {
            Self::Domain(error) => error,
            Self::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
        }
    }
}

impl From<LifecycleLoadError> for ApplyError {
    fn from(error: LifecycleLoadError) -> Self {
        match error {
            LifecycleLoadError::Domain(error) => Self::Domain(error),
            LifecycleLoadError::Driver(error) => Self::Driver(error),
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
impl From<TransactionEventError> for ApplyError {
    fn from(error: TransactionEventError) -> Self {
        match error {
            TransactionEventError::Domain(error) => Self::Domain(error),
            TransactionEventError::Driver(error) => Self::Driver(error),
        }
    }
}

pub(super) struct Receipt {
    pub sandbox: Sandbox,
    event: SandboxEvent,
}

pub(super) async fn load(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
) -> Result<(SandboxUnpublishedRecoverySnapshot, u64), ApplyError> {
    let document = store
        .sandboxes
        .find_one(doc! { "_id": id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let sandbox = read_sandbox(&document).map_err(ApplyError::Domain)?;
    let last_seq = document
        .get_i64("last_seq")
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    let last_seq = unsigned(last_seq, "recovery event sequence")
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?;
    let agents = load_sandbox_agents(store, id, session).await?;
    let generation = sandbox
        .unpublished_recovery_binding()
        .and_then(|binding| binding.provision_generation)
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    for agent in &agents {
        scan_complete_log_with_session(
            store,
            &agent.id,
            agent.conversation.last_seq,
            session,
            |entry| {
                if matches!(entry.kind,
                AgentLogKind::ConversationOpened { runtime_generation, .. }
                | AgentLogKind::RuntimeGenerationRetired { runtime_generation, .. }
                    if runtime_generation >= generation)
                {
                    return Err(SandboxStoreError::InvalidTransaction(
                        "runtime generation was already published",
                    ));
                }
                Ok(())
            },
        )
        .await?;
    }
    let mut cursor = store
        .workspaces
        .find(doc! { "sandbox_id": id.as_str() })
        .sort(doc! { "_id": 1 })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let mut workspaces = Vec::new();
    while let Some(document) = cursor
        .stream(&mut *session)
        .try_next()
        .await
        .map_err(ApplyError::Driver)?
    {
        workspaces.push(read_workspace(&document).map_err(ApplyError::Domain)?);
    }
    Ok((
        SandboxUnpublishedRecoverySnapshot {
            sandbox,
            agents,
            workspaces,
        },
        last_seq,
    ))
}

pub(super) async fn apply(
    store: &MongoSandboxStore,
    request: &SandboxUnpublishedRecoveryRequest,
    session: &mut ClientSession,
) -> Result<Receipt, ApplyError> {
    let (current, last_seq) = load(store, &request.snapshot.sandbox.id, session).await?;
    let mutation =
        apply_unpublished_runtime_recovery(&current, request).map_err(ApplyError::Domain)?;
    validate_in_transaction(store, &current.sandbox.id, last_seq, session).await?;
    // Actual writes to every child serialize native proof with dispatch, cleanup,
    // Workspace setup and lifecycle publication. Preserve every child byte.
    for agent in &current.agents {
        let result = store.agents.update_one(doc! {
            "_id": agent.id.as_str(), "sandbox_id": current.sandbox.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(agent.revision, "recovery Agent revision").map_err(corrupt)?,
            "last_seq": signed(agent.conversation.last_seq, "recovery Agent log sequence").map_err(corrupt)?,
        }, doc! { "$inc": { "sandbox_recovery_fence": 1_i64 } })
            .session(&mut *session).await.map_err(ApplyError::Driver)?;
        if result.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    for workspace in &current.workspaces {
        let result = store.workspaces.update_one(doc! {
            "_id": workspace.id.as_str(), "sandbox_id": current.sandbox.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(workspace.revision, "recovery Workspace revision").map_err(corrupt)?,
        }, doc! { "$inc": { "sandbox_recovery_fence": 1_i64 } })
            .session(&mut *session).await.map_err(ApplyError::Driver)?;
        if result.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    let result = store.sandboxes.update_one(doc! {
        "_id": current.sandbox.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "channel_reuse_cleanup_execution": { "$exists": false },
        "channel_retirement_finalization": { "$exists": false },
        "revision": signed(current.sandbox.revision, "recovery Sandbox revision").map_err(corrupt)?,
        "last_seq": signed(last_seq, "recovery Sandbox event sequence").map_err(corrupt)?,
    }, doc! { "$set": sandbox_replacement_fields(&mutation.sandbox).map_err(ApplyError::Domain)? })
        .session(&mut *session).await.map_err(ApplyError::Driver)?;
    if result.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    crate::store::sandbox_usage::save::record_saved(
        store,
        session,
        &mutation.sandbox,
        current.sandbox.revision,
    )
    .await?;
    let event = append_in_transaction(
        store,
        session,
        &current.sandbox.id,
        request.requested_at.clone(),
        mutation.event,
    )
    .await?;
    Ok(Receipt {
        sandbox: mutation.sandbox,
        event,
    })
}

pub(super) async fn reconcile(
    store: &MongoSandboxStore,
    receipt: &Receipt,
    time: Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let current = store
        .sandboxes
        .find_one(doc! { "_id": receipt.sandbox.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_sandbox)
        .transpose()?;
    if current.as_ref() != Some(&receipt.sandbox) {
        return Ok(false);
    }
    Ok(
        reconcile_event_with_snapshot_session(store, &receipt.event, &mut session)
            .await?
            .is_some(),
    )
}

fn corrupt(error: String) -> ApplyError {
    ApplyError::Domain(SandboxStoreError::Corrupt(error))
}
