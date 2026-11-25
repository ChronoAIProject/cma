use crate::store::agent_log_telemetry::ScanCaller;
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    apply_unpublished_runtime_recovery, AgentLogKind, Sandbox, SandboxEvent,
    SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoverySnapshot,
};
use layer2_core::SandboxId;
use layer3_adapter::recovery_diagnostics::{Observation, Reason, Stage};
use mongodb::bson::{doc, Timestamp};
use mongodb::options::ReadConcern;
use mongodb::ClientSession;
use sha2::{Digest, Sha256};

use super::super::agent_capacity_recovery::source::{capture_segmented, fence_agent};
use super::super::agent_log_integrity::{
    scan_complete_log_with_policy, IntegrityFailurePolicy, TransactionLogError,
};
use super::super::lifecycle_execution::{load_sandbox_agents, LifecycleLoadError};
use super::super::sandbox_events::{
    append_in_transaction, reconcile_event_with_snapshot_session, validate_in_transaction,
    TransactionEventError,
};
use super::super::sandbox_mongo::{signed, unsigned, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::super::workspaces::read_workspace;

mod reopen;
mod scoped;
pub(in crate::store) use scoped::NativeFacts;
pub(super) use scoped::{apply_scoped, load_retained_scoped, load_scoped};

#[derive(Debug)]
pub(in crate::store) enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl ApplyError {
    pub(in crate::store) fn domain(self) -> SandboxStoreError {
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
    load_with_sources(store, id, session, None).await
}

pub(in crate::store) async fn load_reopen_with_sources(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
    sources: Option<&NativeFacts>,
    policy: IntegrityFailurePolicy,
) -> Result<(SandboxUnpublishedRecoverySnapshot, u64, String), ApplyError> {
    load_for_inspection(store, id, session, true, true, sources, policy).await
}

pub(in crate::store) async fn load_suspended_reopen_with_sources(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
    sources: &NativeFacts,
    policy: IntegrityFailurePolicy,
) -> Result<(SandboxUnpublishedRecoverySnapshot, u64, String), ApplyError> {
    load_for_inspection(store, id, session, false, true, Some(sources), policy).await
}

pub(super) async fn load_retained(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
) -> Result<layer2_core::sandbox::RetainedRecoverySnapshot, ApplyError> {
    load_retained_with_sources(store, id, session, None).await
}

async fn load_retained_with_sources(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
    sources: Option<&scoped::NativeFacts>,
) -> Result<layer2_core::sandbox::RetainedRecoverySnapshot, ApplyError> {
    let (snapshot, last_seq, _) = load_for_inspection(
        store,
        id,
        session,
        false,
        false,
        sources,
        IntegrityFailurePolicy::ReportOnly,
    )
    .await?;
    validate_in_transaction(store, id, last_seq, session).await?;
    Ok(layer2_core::sandbox::RetainedRecoverySnapshot {
        sandbox: snapshot.sandbox,
        agents: snapshot.agents,
        workspaces: snapshot.workspaces,
    })
}

async fn load_with_sources(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
    sources: Option<&scoped::NativeFacts>,
) -> Result<(SandboxUnpublishedRecoverySnapshot, u64), ApplyError> {
    load_for_inspection(
        store,
        id,
        session,
        true,
        false,
        sources,
        IntegrityFailurePolicy::Quarantine,
    )
    .await
    .map(|(snapshot, seq, _)| (snapshot, seq))
}

async fn load_for_inspection(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
    require_unpublished: bool,
    require_reopen: bool,
    sources: Option<&scoped::NativeFacts>,
    policy: IntegrityFailurePolicy,
) -> Result<(SandboxUnpublishedRecoverySnapshot, u64, String), ApplyError> {
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
    let agents = match sources {
        Some(sources) => sources.agents.clone(),
        None => load_sandbox_agents(store, id, session).await?,
    };
    let generation = sandbox
        .unpublished_recovery_binding()
        .and_then(|binding| binding.provision_generation);
    if require_unpublished && generation.is_none() {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let mut history_digest = Sha256::new();
    for agent in &agents {
        let mut observation = Observation::new(Stage::PublicationLog);
        let facts = match sources
            .map(|sources| sources.for_agent(agent))
            .transpose()?
            .flatten()
        {
            Some(facts) => Some(facts),
            None => match capture_segmented(store, agent, session).await {
                Ok(source) => source.map(|source| {
                    (
                        source.max_opened_generation(),
                        source.max_retired_generation(),
                    )
                }),
                Err(error) => {
                    observation.finish(match &error {
                        TransactionLogError::Driver(_)
                        | TransactionLogError::Domain(SandboxStoreError::Unavailable(_)) => {
                            Reason::Unavailable
                        }
                        _ => Reason::CorruptHistory,
                    });
                    return Err(error.into());
                }
            },
        };
        if let Some((opened, retired)) = facts {
            if require_reopen {
                let proof = sources
                    .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?
                    .reopen_proof(agent)?;
                // Preserve the original-event digest for legacy Agents. New
                // format histories bind their certified prefix and active leaves.
                history_digest.update((proof.len() as u64).to_be_bytes());
                history_digest.update(proof);
            }
            if require_unpublished
                && generation
                    .is_some_and(|generation| opened >= generation || retired >= generation)
            {
                observation.finish(Reason::PublishedGeneration);
                return Err(ApplyError::Domain(SandboxStoreError::InvalidTransaction(
                    "runtime generation was already published",
                )));
            }
            observation.finish(Reason::Success);
            continue;
        }
        let mut published = false;
        let mut entries = Vec::new();
        let result = if require_unpublished || require_reopen {
            scan_complete_log_with_policy(
                ScanCaller::RecoveryProof,
                store,
                &agent.id,
                agent.conversation.last_seq,
                session,
                policy,
                |entry| {
                    if require_unpublished
                        && matches!(entry.kind,
                AgentLogKind::ConversationOpened { runtime_generation, .. }
                | AgentLogKind::RuntimeGenerationRetired { runtime_generation, .. }
                    if generation.is_some_and(|generation| runtime_generation >= generation))
                    {
                        published = true;
                        return Err(SandboxStoreError::InvalidTransaction(
                            "runtime generation was already published",
                        ));
                    }
                    if require_reopen {
                        let bytes = serde_json::to_vec(&entry).map_err(|_| {
                            SandboxStoreError::Corrupt("recovery history encoding failed".into())
                        })?;
                        history_digest.update((bytes.len() as u64).to_be_bytes());
                        history_digest.update(bytes);
                        entries.push(entry);
                    }
                    Ok(())
                },
            )
            .await
        } else {
            // Assessment is available before consent and even with admission
            // disabled. Existing quarantine is a refusal, never a reason to write.
            super::super::agent_log_integrity::read::scan_complete_log_with_session(
                ScanCaller::RecoveryProof,
                store,
                agent,
                session,
            )
            .await
        };
        observation.finish(match &result {
            Ok(_) => Reason::Success,
            Err(TransactionLogError::Driver(_)) => Reason::Unavailable,
            Err(_) if published => Reason::PublishedGeneration,
            Err(_) => Reason::CorruptHistory,
        });
        let summary = result?;
        if require_reopen {
            use super::super::agent_log_integrity::{
                audit_checkpoint, ensure_reopen_capacity, SemanticCheckpoint,
            };
            // Audit before refusal so a disproved trusted checkpoint is revoked.
            if policy == IntegrityFailurePolicy::Quarantine {
                audit_checkpoint(store, agent, &entries, session).await?;
            }
            SemanticCheckpoint::validate_retained_semantics(agent, &entries)
                .map_err(ApplyError::Domain)?;
            ensure_reopen_capacity(&summary).map_err(ApplyError::Domain)?;
        }
    }
    let workspaces = if let Some(sources) = sources {
        sources.workspaces.clone()
    } else {
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
        workspaces
    };
    Ok((
        SandboxUnpublishedRecoverySnapshot {
            sandbox,
            agents,
            workspaces,
        },
        last_seq,
        format!("{:x}", history_digest.finalize()),
    ))
}

pub(super) async fn apply(
    store: &MongoSandboxStore,
    request: &SandboxUnpublishedRecoveryRequest,
    session: &mut ClientSession,
) -> Result<Receipt, ApplyError> {
    apply_with_sources(store, request, session, None).await
}

async fn apply_with_sources(
    store: &MongoSandboxStore,
    request: &SandboxUnpublishedRecoveryRequest,
    session: &mut ClientSession,
    sources: Option<&scoped::NativeFacts>,
) -> Result<Receipt, ApplyError> {
    let (current, last_seq) =
        load_with_sources(store, &request.snapshot.sandbox.id, session, sources).await?;
    // A failed pause can retain a recoverable runtime binding after losing its
    // pause operation. Recovery starts a new runtime commitment, so discovery
    // or an earlier sweep result cannot authorize it under a current stop.
    // This recovers an already-admitted runtime, so operator STOP permits
    // continuation while the spend gate still applies to the new liability.
    crate::store::budget::require_open(
        store,
        crate::store::budget::now_seconds(),
        layer2_core::admission::AdmissionCause::Continuation,
        session,
    )
    .await
    .map_err(|error| match error {
        crate::store::agent_conversation_store::ApplyError::Domain(error) => {
            ApplyError::Domain(error)
        }
        crate::store::agent_conversation_store::ApplyError::Driver(error) => {
            ApplyError::Driver(error)
        }
    })?;
    let mutation =
        apply_unpublished_runtime_recovery(&current, request).map_err(ApplyError::Domain)?;
    if let Some(release) = &request.released_runtime {
        if release.agent != request.target_runtime_version {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        store
            .fence_released_admission(
                release,
                layer2_core::released_runtime::ReleasedLane::Agent,
                session,
            )
            .await
            .map_err(ApplyError::Domain)?;
    }
    validate_in_transaction(store, &current.sandbox.id, last_seq, session).await?;
    // Actual writes to every child serialize native proof with dispatch, cleanup,
    // Workspace setup and lifecycle publication. Preserve every child byte.
    for agent in &current.agents {
        fence_agent(store, agent, "sandbox_recovery_fence", session).await?;
    }
    for workspace in &current.workspaces {
        let result = store.workspaces.update_one(crate::store::work_schema::query(doc! {
            "_id": workspace.id.as_str(), "sandbox_id": current.sandbox.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(workspace.revision, "recovery Workspace revision").map_err(corrupt)?,
        }), doc! { "$inc": { "sandbox_recovery_fence": 1_i64 } })
            .session(&mut *session).await.map_err(ApplyError::Driver)?;
        if result.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    let result = store.sandboxes.update_one(crate::store::work_schema::query(doc! {
        "_id": current.sandbox.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "channel_reuse_cleanup_execution": { "$exists": false },
        "channel_retirement_finalization": { "$exists": false },
        "revision": signed(current.sandbox.revision, "recovery Sandbox revision").map_err(corrupt)?,
        "last_seq": signed(last_seq, "recovery Sandbox event sequence").map_err(corrupt)?,
    }), crate::store::work_lease_update::replacement(sandbox_replacement_fields(&mutation.sandbox).map_err(ApplyError::Domain)?))
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
    if !crate::store::work_lease_update::sandbox_matches(current.as_ref(), &receipt.sandbox) {
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
