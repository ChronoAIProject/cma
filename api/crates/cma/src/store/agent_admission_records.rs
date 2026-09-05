//! Replay loading and commit reconciliation for Mongo agent admission.

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

pub(super) struct CreatedReceipt {
    pub(super) creation: AgentCreation,
    pub(super) workspace: Workspace,
    pub(super) sandbox: Sandbox,
    pub(super) worktree_event: SandboxEvent,
    pub(super) agent_event: SandboxEvent,
}

pub(super) enum AdmissionReceipt {
    Created(Box<CreatedReceipt>),
    Existing(Box<AgentCreation>),
}

impl AdmissionReceipt {
    pub(super) fn result(&self) -> AgentAdmissionResult {
        match self {
            Self::Created(receipt) => AgentAdmissionResult::Created(receipt.creation.clone()),
            Self::Existing(creation) => AgentAdmissionResult::Existing(creation.as_ref().clone()),
        }
    }
}

pub(super) enum LoadError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
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
    load_replay_with_session(store, admission, session, ReplayRead::Transaction).await
}

#[derive(Clone, Copy)]
enum ReplayRead {
    Transaction,
    Snapshot,
}

async fn load_replay_with_session(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    session: &mut ClientSession,
    read: ReplayRead,
) -> Result<Option<AdmissionReceipt>, LoadError> {
    let agent_find = store.agents.find_one(doc! {
        "owner": admission.owner.as_str(),
        "creation_key": &admission.creation_key,
    });
    let agent_find = match read {
        ReplayRead::Transaction => agent_find,
        ReplayRead::Snapshot => agent_find.read_concern(ReadConcern::snapshot()),
    };
    let Some(agent_document) = agent_find
        .session(&mut *session)
        .await
        .map_err(LoadError::Driver)?
    else {
        return Ok(None);
    };
    let agent = read_agent(&agent_document).map_err(LoadError::Domain)?;
    let worktree_find = store
        .worktrees
        .find_one(doc! { "_id": agent.worktree_id.as_str() });
    let worktree_find = match read {
        ReplayRead::Transaction => worktree_find,
        ReplayRead::Snapshot => worktree_find.read_concern(ReadConcern::snapshot()),
    };
    let worktree_document = worktree_find
        .session(&mut *session)
        .await
        .map_err(LoadError::Driver)?;
    let last_seq = agent.conversation.last_seq;
    let mut initial_log = None;
    match read {
        ReplayRead::Transaction => {
            scan_complete_log_with_session(store, &agent.id, last_seq, session, |entry| {
                if initial_log.is_none() {
                    initial_log = Some(entry);
                }
                Ok(())
            })
            .await?;
        }
        ReplayRead::Snapshot => {
            scan_complete_log_with_snapshot_session(store, &agent.id, last_seq, session, |entry| {
                if initial_log.is_none() {
                    initial_log = Some(entry);
                }
                Ok(())
            })
            .await?;
        }
    }
    let creation =
        assemble_creation(agent, worktree_document, initial_log).map_err(LoadError::Domain)?;
    classify_replay(admission, creation)
        .map(Some)
        .map_err(LoadError::Domain)
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
    load_replay_with_session(store, admission, &mut session, ReplayRead::Snapshot)
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
    let mut initial_log = None;
    scan_complete_log_with_snapshot_session(
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

    Ok(workspace.as_ref() == Some(&receipt.workspace)
        && sandbox.as_ref() == Some(&receipt.sandbox)
        && worktree.as_ref() == Some(&receipt.creation.worktree)
        && agent.as_ref() == Some(&receipt.creation.agent)
        && initial_log_matches
        && worktree_event_matches
        && agent_event_matches)
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
