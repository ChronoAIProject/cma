//! Replay loading and commit reconciliation for Mongo agent admission.

use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionResult, AgentCreation, SandboxStoreError,
};
use layer2_core::sandbox::{Sandbox, SandboxEvent, Workspace};
use mongodb::bson::{doc, Document};
use mongodb::ClientSession;

use super::agent_log::{read_entry, reconcile_entry};
use super::agents::read_agent;
use super::sandbox_events::reconcile_event;
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

pub(super) async fn replay_in_transaction(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    session: &mut ClientSession,
) -> Result<Option<AdmissionReceipt>, LoadError> {
    let Some(agent_document) = store
        .agents
        .find_one(doc! {
            "owner": admission.owner.as_str(),
            "creation_key": &admission.creation_key,
        })
        .session(&mut *session)
        .await
        .map_err(LoadError::Driver)?
    else {
        return Ok(None);
    };
    let agent = read_agent(&agent_document).map_err(LoadError::Domain)?;
    let worktree_document = store
        .worktrees
        .find_one(doc! { "_id": agent.worktree_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(LoadError::Driver)?;
    let log_document = store
        .agent_log
        .find_one(doc! { "agent_id": agent.id.as_str(), "seq": 1_i64 })
        .session(session)
        .await
        .map_err(LoadError::Driver)?;
    let creation =
        assemble_creation(agent, worktree_document, log_document).map_err(LoadError::Domain)?;
    classify_replay(admission, creation)
        .map(Some)
        .map_err(LoadError::Domain)
}

pub(super) async fn replay_after_duplicate(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
) -> Result<Option<AgentAdmissionResult>, SandboxStoreError> {
    let Some(agent_document) = store
        .agents
        .find_one(doc! {
            "owner": admission.owner.as_str(),
            "creation_key": &admission.creation_key,
        })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    else {
        return Ok(None);
    };
    let agent = read_agent(&agent_document)?;
    let worktree_document = store
        .worktrees
        .find_one(doc! { "_id": agent.worktree_id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let log_document = store
        .agent_log
        .find_one(doc! { "agent_id": agent.id.as_str(), "seq": 1_i64 })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let creation = assemble_creation(agent, worktree_document, log_document)?;
    classify_replay(admission, creation).map(|receipt| Some(receipt.result()))
}

fn assemble_creation(
    agent: layer2_core::sandbox::Agent,
    worktree_document: Option<Document>,
    log_document: Option<Document>,
) -> Result<AgentCreation, SandboxStoreError> {
    let worktree = worktree_document
        .as_ref()
        .map(read_worktree)
        .transpose()?
        .ok_or_else(|| SandboxStoreError::Corrupt("agent worktree is missing".into()))?;
    let initial_log = log_document
        .as_ref()
        .map(read_entry)
        .transpose()?
        .ok_or_else(|| SandboxStoreError::Corrupt("agent initial log is missing".into()))?;
    Ok(AgentCreation {
        agent,
        worktree,
        initial_log,
    })
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
) -> Result<bool, SandboxStoreError> {
    let workspace = store
        .workspaces
        .find_one(doc! { "_id": receipt.workspace.id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_workspace)
        .transpose()?;
    let sandbox = store
        .sandboxes
        .find_one(doc! { "_id": receipt.sandbox.id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_sandbox)
        .transpose()?;
    let worktree = store
        .worktrees
        .find_one(doc! { "_id": receipt.creation.worktree.id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_worktree)
        .transpose()?;
    let agent = store
        .agents
        .find_one(doc! { "_id": receipt.creation.agent.id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_agent)
        .transpose()?;

    Ok(workspace
        .as_ref()
        .is_some_and(|actual| workspace_fence_matches(actual, &receipt.workspace))
        && sandbox
            .as_ref()
            .is_some_and(|actual| sandbox_fence_matches(actual, &receipt.sandbox))
        && worktree.as_ref() == Some(&receipt.creation.worktree)
        && agent.as_ref() == Some(&receipt.creation.agent)
        && reconcile_entry(store, &receipt.creation.initial_log)
            .await?
            .is_some()
        && reconcile_event(store, &receipt.worktree_event)
            .await?
            .is_some()
        && reconcile_event(store, &receipt.agent_event)
            .await?
            .is_some())
}

fn workspace_fence_matches(actual: &Workspace, expected: &Workspace) -> bool {
    actual.id == expected.id
        && actual.next_worktree_number >= expected.next_worktree_number
        && actual.revision >= expected.revision
}

fn sandbox_fence_matches(actual: &Sandbox, expected: &Sandbox) -> bool {
    actual.id == expected.id
        && actual.revision >= expected.revision
        && (actual.revision > expected.revision
            || (actual.updated_at == expected.updated_at
                && actual.last_activity_at == expected.last_activity_at))
}
