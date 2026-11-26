//! Replay loading and commit reconciliation for Mongo agent admission.

use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionResult, AgentCreation, SandboxStoreError,
};
use layer2_core::sandbox::{Sandbox, SandboxEvent, Workspace};
use mongodb::bson::{doc, Document, Timestamp};
use mongodb::options::ReadConcern;
use mongodb::{ClientSession, Collection};

use super::agent_log::{scan_complete_log_with_session, scan_complete_log_with_snapshot_session};
use super::agent_log_integrity::TransactionLogError;
use super::agents::read_agent;
use super::sandbox_events::read_event;
use super::sandbox_mongo::MongoSandboxStore;
use super::sandboxes::read_sandbox;
use super::workspaces::read_workspace;
use super::worktrees::read_worktree;

mod source;
mod unknown_existing;
pub(super) use unknown_existing::reconcile_existing;

pub(super) struct CreatedReceipt {
    pub(super) creation: AgentCreation,
    pub(super) provenance: layer2_core::sandbox::AgentCreationProvenance,
    pub(super) workspace: Workspace,
    pub(super) sandbox: Sandbox,
    pub(super) revival_event: Option<SandboxEvent>,
    pub(super) worktree_event: SandboxEvent,
    pub(super) agent_event: SandboxEvent,
}

pub(super) struct ReusedReceipt {
    pub creation: AgentCreation,
    pub recorded: super::agent_log_segments::writer_commit::Recorded,
}

pub(super) enum AdmissionReceipt {
    Created(Box<CreatedReceipt>),
    Existing(Box<AgentCreation>),
    Reused(Box<ReusedReceipt>),
}

impl AdmissionReceipt {
    pub(super) fn result(&self) -> AgentAdmissionResult {
        match self {
            Self::Reused(receipt) => AgentAdmissionResult::Existing(receipt.creation.clone()),
            Self::Created(receipt) => AgentAdmissionResult::Created(receipt.creation.clone()),
            Self::Existing(creation) => AgentAdmissionResult::Existing(creation.as_ref().clone()),
        }
    }
}

pub(super) enum LoadError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<SandboxStoreError> for LoadError {
    fn from(error: SandboxStoreError) -> Self {
        Self::Domain(error)
    }
}

impl From<TransactionLogError> for LoadError {
    fn from(error: TransactionLogError) -> Self {
        match error {
            TransactionLogError::Domain(error) => Self::Domain(error),
            TransactionLogError::Driver(error) => Self::Driver(error),
        }
    }
}

pub(super) async fn replay_in_transaction(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    session: &mut ClientSession,
) -> Result<Option<AdmissionReceipt>, LoadError> {
    Box::pin(load_replay_with_session(
        store,
        admission,
        session,
        ReplayRead::Transaction,
    ))
    .await
}

pub(super) async fn replay_at_snapshot(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    session: &mut ClientSession,
) -> Result<Option<AgentAdmissionResult>, SandboxStoreError> {
    Box::pin(load_replay_with_session(
        store,
        admission,
        session,
        ReplayRead::Snapshot,
    ))
    .await
    .map(|receipt| receipt.map(|receipt| receipt.result()))
    .map_err(load_error)
}

#[derive(Clone, Copy)]
enum ReplayRead {
    Transaction,
    Snapshot,
    UncertainSnapshot,
}

async fn load_replay_with_session(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    session: &mut ClientSession,
    read: ReplayRead,
) -> Result<Option<AdmissionReceipt>, LoadError> {
    let binding_id = binding_id(admission).map_err(LoadError::Domain)?;
    let binding_find = store
        .agent_reuse_bindings
        .find_one(doc! { "_id": &binding_id });
    let binding_find = match read {
        ReplayRead::Transaction => binding_find,
        ReplayRead::Snapshot | ReplayRead::UncertainSnapshot => {
            binding_find.read_concern(ReadConcern::snapshot())
        }
    };
    let binding = binding_find
        .session(&mut *session)
        .await
        .map_err(LoadError::Driver)?;
    let mut agent_query = doc! {
        "owner": admission.owner.as_str(),
        "creation_key": &admission.creation_key,
    };
    if let Some(binding) = &binding {
        let version = binding.get_i64("document_schema_version").ok();
        if !matches!(version, Some(1 | 2))
            || (version == Some(2)) != binding.contains_key("log_publication")
            || binding.get_str("owner").ok() != Some(admission.owner.as_str())
            || binding.get_str("creation_key").ok() != Some(admission.creation_key.as_str())
        {
            return Err(LoadError::Domain(SandboxStoreError::Corrupt(
                "Agent reuse binding is invalid".into(),
            )));
        }
        if binding.get_str("fingerprint").ok()
            != Some(admission.fingerprint().map_err(LoadError::Domain)?.as_str())
        {
            return Err(LoadError::Domain(SandboxStoreError::AgentCreationConflict));
        }
        agent_query = doc! { "_id": binding.get_str("agent_id").map_err(|_| LoadError::Domain(SandboxStoreError::Corrupt("Agent reuse binding has no Agent".into())))?, "owner": admission.owner.as_str() };
    }
    let agent_find = store.agents.find_one(agent_query);
    let agent_find = match read {
        ReplayRead::Transaction => agent_find,
        ReplayRead::Snapshot | ReplayRead::UncertainSnapshot => {
            agent_find.read_concern(ReadConcern::snapshot())
        }
    };
    let Some(agent_document) = agent_find
        .session(&mut *session)
        .await
        .map_err(LoadError::Driver)?
    else {
        return if binding.is_some() {
            Err(LoadError::Domain(SandboxStoreError::Corrupt(
                "Agent reuse binding target is missing".into(),
            )))
        } else {
            Ok(None)
        };
    };
    let agent = read_agent(&agent_document).map_err(LoadError::Domain)?;
    if let Some(binding) = &binding {
        Box::pin(super::agent_log_segments::admission::replay::require(
            store,
            admission,
            &agent,
            &agent_document,
            binding,
            session,
            matches!(read, ReplayRead::Snapshot | ReplayRead::UncertainSnapshot),
        ))
        .await?;
    }

    match read {
        ReplayRead::Transaction => {
            super::trigger_first_messages::require_command_receipt(store, &agent, session).await
        }
        ReplayRead::Snapshot | ReplayRead::UncertainSnapshot => {
            super::trigger_first_messages::require_command_receipt_snapshot(store, &agent, session)
                .await
        }
    }
    .map_err(LoadError::Domain)?;

    let worktree_find = store
        .worktrees
        .find_one(doc! { "_id": agent.worktree_id.as_str() });
    let worktree_find = match read {
        ReplayRead::Transaction => worktree_find,
        ReplayRead::Snapshot | ReplayRead::UncertainSnapshot => {
            worktree_find.read_concern(ReadConcern::snapshot())
        }
    };
    let worktree_document = worktree_find
        .session(&mut *session)
        .await
        .map_err(LoadError::Driver)?;
    let initial_log = match super::agent_log_segment_reads::range_with_snapshot(
        store,
        &agent,
        session,
        0,
        1,
        matches!(read, ReplayRead::Snapshot | ReplayRead::UncertainSnapshot),
    )
    .await
    .map_err(LoadError::Domain)?
    {
        Some(entries) => entries.into_iter().next(),
        None => source::legacy_initial(store, &agent, session, read).await?,
    };
    let creation =
        assemble_creation(agent, worktree_document, initial_log).map_err(LoadError::Domain)?;
    if binding.is_some() {
        creation.validate_replay().map_err(LoadError::Domain)?;
        if creation.agent.workspace_id != admission.workspace_id
            || creation.agent.profile != admission.profile
        {
            return Err(LoadError::Domain(SandboxStoreError::Corrupt(
                "Agent reuse binding lineage disagrees".into(),
            )));
        }
        return Ok(Some(AdmissionReceipt::Existing(Box::new(creation))));
    }
    classify_replay(admission, creation)
        .map(Some)
        .map_err(LoadError::Domain)
}

pub(super) fn binding_id(admission: &AgentAdmission) -> Result<String, SandboxStoreError> {
    layer2_core::profiles::content_digest(&(
        admission.owner.as_str(),
        admission.creation_key.as_str(),
    ))
    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
}

pub(super) async fn replay_after_duplicate(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    snapshot_time: Timestamp,
) -> Result<Option<AgentAdmissionResult>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    Box::pin(load_replay_with_session(
        store,
        admission,
        &mut session,
        ReplayRead::Snapshot,
    ))
    .await
    .map(|receipt| receipt.map(|receipt| receipt.result()))
    .map_err(load_error)
}

fn assemble_creation(
    agent: layer2_core::sandbox::Agent,
    worktree_document: Option<Document>,
    initial_log: Option<layer2_core::sandbox::AgentLogEntry>,
) -> Result<AgentCreation, SandboxStoreError> {
    let worktree = worktree_document
        .as_ref()
        .map(read_worktree)
        .transpose()?
        .ok_or_else(|| SandboxStoreError::Corrupt("agent worktree is missing".into()))?;
    let initial_log = initial_log
        .ok_or_else(|| SandboxStoreError::Corrupt("agent initial log is missing".into()))?;
    Ok(AgentCreation {
        agent,
        worktree,
        initial_log,
    })
}

fn load_error(error: LoadError) -> SandboxStoreError {
    match error {
        LoadError::Domain(error) => error,
        LoadError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}

fn classify_replay(
    admission: &AgentAdmission,
    creation: AgentCreation,
) -> Result<AdmissionReceipt, SandboxStoreError> {
    if admission.matches(&creation)? {
        Ok(AdmissionReceipt::Existing(Box::new(creation)))
    } else {
        Err(SandboxStoreError::AgentCreationConflict)
    }
}

pub(super) async fn reconcile_created(
    store: &MongoSandboxStore,
    receipt: &CreatedReceipt,
    snapshot_time: Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let workspace = find_at_snapshot(
        &store.workspaces,
        doc! { "_id": receipt.workspace.id.as_str() },
        &mut session,
    )
    .await?
    .as_ref()
    .map(read_workspace)
    .transpose()?;
    let sandbox = find_at_snapshot(
        &store.sandboxes,
        doc! { "_id": receipt.sandbox.id.as_str() },
        &mut session,
    )
    .await?
    .as_ref()
    .map(read_sandbox)
    .transpose()?;
    let worktree = find_at_snapshot(
        &store.worktrees,
        doc! { "_id": receipt.creation.worktree.id.as_str() },
        &mut session,
    )
    .await?
    .as_ref()
    .map(read_worktree)
    .transpose()?;
    let agent = find_at_snapshot(
        &store.agents,
        doc! { "_id": receipt.creation.agent.id.as_str() },
        &mut session,
    )
    .await?
    .as_ref()
    .map(read_agent)
    .transpose()?;
    if let Some(agent) = &agent {
        super::trigger_first_messages::require_command_receipt_snapshot(store, agent, &mut session)
            .await?;
    }
    let provenance = find_at_snapshot(
        &store.agent_creation_provenance,
        doc! { "_id": receipt.creation.agent.id.as_str() },
        &mut session,
    )
    .await?
    .as_ref()
    .map(super::agent_provenance::read)
    .transpose()?;
    let provenance_matches = exact_artifact_matches(provenance.as_ref(), &receipt.provenance)?;
    let mut initial_log = None;
    scan_complete_log_with_snapshot_session(
        ScanCaller::UnknownCommitProof,
        store,
        &receipt.creation.agent.id,
        receipt.creation.agent.conversation.last_seq,
        &mut session,
        |entry| {
            if entry.seq == receipt.creation.initial_log.seq {
                initial_log = Some(entry);
            }
            Ok(())
        },
    )
    .await
    .map_err(|error| load_error(error.into()))?;
    let worktree_event = find_at_snapshot(
        &store.sandbox_events,
        doc! { "_id": sequence_id(
            receipt.worktree_event.sandbox_id.as_str(),
            receipt.worktree_event.seq,
        ) },
        &mut session,
    )
    .await?
    .as_ref()
    .map(read_event)
    .transpose()?;
    let agent_event = find_at_snapshot(
        &store.sandbox_events,
        doc! { "_id": sequence_id(
            receipt.agent_event.sandbox_id.as_str(),
            receipt.agent_event.seq,
        ) },
        &mut session,
    )
    .await?
    .as_ref()
    .map(read_event)
    .transpose()?;
    let initial_log_matches =
        exact_artifact_matches(initial_log.as_ref(), &receipt.creation.initial_log)?;
    let worktree_event_matches =
        exact_artifact_matches(worktree_event.as_ref(), &receipt.worktree_event)?;
    let agent_event_matches = exact_artifact_matches(agent_event.as_ref(), &receipt.agent_event)?;
    let revival_matches = match receipt.revival_event.as_ref() {
        Some(expected) => {
            let actual = find_at_snapshot(
                &store.sandbox_events,
                doc! {
                    "_id": sequence_id(expected.sandbox_id.as_str(), expected.seq),
                },
                &mut session,
            )
            .await?
            .as_ref()
            .map(read_event)
            .transpose()?;
            exact_artifact_matches(actual.as_ref(), expected)?
        }
        None => true,
    };

    Ok(workspace.as_ref() == Some(&receipt.workspace)
        && sandbox.as_ref() == Some(&receipt.sandbox)
        && worktree.as_ref() == Some(&receipt.creation.worktree)
        && agent.as_ref() == Some(&receipt.creation.agent)
        && provenance_matches
        && initial_log_matches
        && worktree_event_matches
        && agent_event_matches
        && revival_matches)
}

fn exact_artifact_matches<T: PartialEq>(
    actual: Option<&T>,
    expected: &T,
) -> Result<bool, SandboxStoreError> {
    match actual {
        Some(actual) if actual == expected => Ok(true),
        Some(_) => Err(SandboxStoreError::SequenceConflict),
        None => Ok(false),
    }
}

async fn find_at_snapshot(
    collection: &Collection<Document>,
    filter: Document,
    session: &mut ClientSession,
) -> Result<Option<Document>, SandboxStoreError> {
    collection
        .find_one(filter)
        .read_concern(ReadConcern::snapshot())
        .session(session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))
}

fn sequence_id(id: &str, seq: u64) -> String {
    format!("{id}:{seq:020}")
}
