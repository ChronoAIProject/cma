//! Snapshot loading and write-conflict fencing for agent control lineage.

use layer2_core::sandbox::store::{AgentControlStoreError, AgentControlTarget, SandboxStoreError};
use layer2_core::sandbox::{Agent, Sandbox, Workspace, Worktree};
use mongodb::bson::doc;
use mongodb::options::ReadConcern;

use super::agent_control_receipt::{read_control_receipts, StoredControlReceipts};
use super::agents::read_agent;
use super::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::sandboxes::read_sandbox;
use super::work_claim_authority::without_ledger_maintenance;
use super::workspaces::read_workspace;
use super::worktrees::read_worktree;

const CONTROL_MARKER_FIELD: &str = "last_agent_control_operation";

#[derive(Debug)]
pub(super) enum LineageError {
    Domain(AgentControlStoreError),
    ReceiptCorrupt(String),
    Driver(mongodb::error::Error),
}

pub(super) struct ControlSnapshot {
    pub(super) row: mongodb::bson::Document,
    pub(super) agent: Agent,
    pub(super) receipts: StoredControlReceipts,
    pub(super) worktree: Worktree,
    pub(super) workspace: Workspace,
    pub(super) sandbox: Sandbox,
}

pub(super) async fn load_control_snapshot(
    store: &MongoSandboxStore,
    target: &AgentControlTarget,
    session: &mut mongodb::ClientSession,
    snapshot_read: bool,
) -> Result<ControlSnapshot, LineageError> {
    let find = store
        .agents
        .find_one(doc! { "_id": target.agent_id.as_str() });
    let find = if snapshot_read {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let document = find
        .session(&mut *session)
        .await
        .map_err(LineageError::Driver)?
        .ok_or(LineageError::Domain(AgentControlStoreError::NotFound))?;
    let agent = read_agent(&document)
        .map_err(store_error)
        .map_err(LineageError::Domain)?;
    validate_target(&agent, target)?;
    let receipts = read_control_receipts(&document, &agent).map_err(|error| match error {
        AgentControlStoreError::Corrupt(detail) => LineageError::ReceiptCorrupt(detail),
        other => LineageError::Domain(other),
    })?;
    let (worktree, workspace, sandbox) =
        load_lineage(store, &agent, session, snapshot_read).await?;
    Ok(ControlSnapshot {
        row: document,
        agent,
        receipts,
        worktree,
        workspace,
        sandbox,
    })
}

fn validate_target(agent: &Agent, target: &AgentControlTarget) -> Result<(), LineageError> {
    if agent.id != target.agent_id {
        return Err(corrupt("agent storage identity does not match its key"));
    }
    if agent.owner != target.owner {
        return Err(LineageError::Domain(AgentControlStoreError::NotFound));
    }
    if agent.revision == 0 || agent.work.next_fence == 0 {
        return Err(corrupt("agent revision or work fence is invalid"));
    }
    if agent.worktree_id != target.worktree_id
        || agent.workspace_id != target.workspace_id
        || agent.sandbox_id != target.sandbox_id
    {
        return Err(LineageError::Domain(AgentControlStoreError::Conflict));
    }
    Ok(())
}

async fn load_lineage(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut mongodb::ClientSession,
    snapshot_read: bool,
) -> Result<(Worktree, Workspace, Sandbox), LineageError> {
    let find = store
        .worktrees
        .find_one(doc! { "_id": agent.worktree_id.as_str() });
    let find = if snapshot_read {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let worktree = find
        .session(&mut *session)
        .await
        .map_err(LineageError::Driver)?
        .as_ref()
        .map(read_worktree)
        .transpose()
        .map_err(store_error)
        .map_err(LineageError::Domain)?
        .ok_or_else(|| corrupt("agent worktree is missing"))?;

    let find = store
        .workspaces
        .find_one(doc! { "_id": agent.workspace_id.as_str() });
    let find = if snapshot_read {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let workspace = find
        .session(&mut *session)
        .await
        .map_err(LineageError::Driver)?
        .as_ref()
        .map(read_workspace)
        .transpose()
        .map_err(store_error)
        .map_err(LineageError::Domain)?
        .ok_or_else(|| corrupt("agent workspace is missing"))?;

    let find = store
        .sandboxes
        .find_one(doc! { "_id": agent.sandbox_id.as_str() });
    let find = if snapshot_read {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let sandbox = find
        .session(session)
        .await
        .map_err(LineageError::Driver)?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(store_error)
        .map_err(LineageError::Domain)?
        .ok_or_else(|| corrupt("agent sandbox is missing"))?;

    if worktree.id != agent.worktree_id
        || worktree.agent_id.as_ref() != Some(&agent.id)
        || worktree.workspace_id != agent.workspace_id
        || worktree.sandbox_id != agent.sandbox_id
        || worktree.owner != agent.owner
        || workspace.id != agent.workspace_id
        || workspace.sandbox_id != agent.sandbox_id
        || workspace.owner != agent.owner
        || sandbox.id != agent.sandbox_id
        || sandbox.owner != agent.owner
    {
        return Err(corrupt("agent lineage does not match its parents"));
    }
    Ok((worktree, workspace, sandbox))
}

pub(super) async fn fence_control_lineage(
    store: &MongoSandboxStore,
    snapshot: &ControlSnapshot,
    operation_id: &str,
    session: &mut mongodb::ClientSession,
) -> Result<(), LineageError> {
    fence_lineage(store, snapshot, operation_id, session, false).await
}

pub(super) async fn fence_restart_lineage(
    store: &MongoSandboxStore,
    snapshot: &ControlSnapshot,
    operation_id: &str,
    session: &mut mongodb::ClientSession,
) -> Result<(), LineageError> {
    fence_lineage(store, snapshot, operation_id, session, true).await
}

async fn fence_lineage(
    store: &MongoSandboxStore,
    snapshot: &ControlSnapshot,
    operation_id: &str,
    session: &mut mongodb::ClientSession,
    requires_active_parent: bool,
) -> Result<(), LineageError> {
    let worktree_revision = signed(snapshot.worktree.revision, "worktree revision")
        .map_err(|_| LineageError::Domain(AgentControlStoreError::Conflict))?;
    let result = store
        .worktrees
        .update_one(
            doc! {
                "_id": snapshot.worktree.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": worktree_revision,
                "workspace_id": snapshot.agent.workspace_id.as_str(),
                "sandbox_id": snapshot.agent.sandbox_id.as_str(),
                "owner": snapshot.agent.owner.as_str(),
                "record.agent_id": snapshot.agent.id.as_str(),
            },
            doc! { "$set": { CONTROL_MARKER_FIELD: operation_id } },
        )
        .session(&mut *session)
        .await
        .map_err(LineageError::Driver)?;
    require_match(result.matched_count)?;

    let workspace_revision = signed(snapshot.workspace.revision, "workspace revision")
        .map_err(|_| LineageError::Domain(AgentControlStoreError::Conflict))?;
    let workspace_filter = active_parent_filter(
        doc! {
            "_id": snapshot.workspace.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": workspace_revision,
            "sandbox_id": snapshot.agent.sandbox_id.as_str(),
            "owner": snapshot.agent.owner.as_str(),
        },
        requires_active_parent,
    );
    let result = store
        .workspaces
        .update_one(
            workspace_filter,
            doc! { "$set": { CONTROL_MARKER_FIELD: operation_id } },
        )
        .session(&mut *session)
        .await
        .map_err(LineageError::Driver)?;
    require_match(result.matched_count)?;

    let sandbox_revision = signed(snapshot.sandbox.revision, "sandbox revision")
        .map_err(|_| LineageError::Domain(AgentControlStoreError::Conflict))?;
    let mut sandbox_filter = active_parent_filter(
        doc! {
            "_id": snapshot.sandbox.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": sandbox_revision,
            "owner": snapshot.agent.owner.as_str(),
        },
        requires_active_parent,
    );
    if requires_active_parent {
        sandbox_filter.insert(
            super::native_retirement_action::FIELD,
            doc! {"$exists": false},
        );
    }
    let result = store
        .sandboxes
        .update_one(
            without_ledger_maintenance(sandbox_filter),
            doc! { "$set": { CONTROL_MARKER_FIELD: operation_id } },
        )
        .session(session)
        .await
        .map_err(LineageError::Driver)?;
    require_match(result.matched_count)
}

fn active_parent_filter(
    mut filter: mongodb::bson::Document,
    required: bool,
) -> mongodb::bson::Document {
    if required {
        filter.insert("channel_reuse_cleanup_execution", doc! {"$exists": false});
        filter.insert(
            "owned_workspace_cleanup_admission_v1",
            doc! {"$exists": false},
        );
    }
    crate::store::work_schema::query(filter)
}

fn require_match(matched: u64) -> Result<(), LineageError> {
    if matched == 1 {
        Ok(())
    } else {
        Err(LineageError::Domain(AgentControlStoreError::Conflict))
    }
}

fn corrupt(detail: &str) -> LineageError {
    LineageError::Domain(AgentControlStoreError::Corrupt(detail.into()))
}

fn store_error(error: SandboxStoreError) -> AgentControlStoreError {
    match error {
        SandboxStoreError::NotFound => AgentControlStoreError::NotFound,
        SandboxStoreError::Corrupt(detail) => AgentControlStoreError::Corrupt(detail),
        other => AgentControlStoreError::Unavailable(other.to_string()),
    }
}
