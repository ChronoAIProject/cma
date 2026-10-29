//! Recovery snapshots, quota/cleanup fences and exact ambiguous-commit evidence.
use super::{
    agents::{agent_replacement_fields, read_agent},
    sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION},
    sandboxes::read_sandbox,
    shared_child_capacity::CapacityError,
    workspaces::{read_workspace, workspace_replacement_fields},
    worktrees::{read_worktree, worktree_document, worktree_replacement_fields},
};
use layer2_core::sandbox::{
    store::{AgentControlStoreError as Domain, SandboxStoreError},
    Agent, AgentWorktreeRecoveryMutation, Sandbox, Workspace, Worktree,
};
use layer2_core::{AgentId, Caller, Capability};
use mongodb::{
    bson::{doc, Document, Timestamp},
    options::ReadConcern,
    ClientSession,
};

pub(super) enum ApplyError {
    Domain(Domain),
    Driver(mongodb::error::Error),
}
impl From<Domain> for ApplyError {
    fn from(error: Domain) -> Self {
        Self::Domain(error)
    }
}
impl From<mongodb::error::Error> for ApplyError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}
impl From<SandboxStoreError> for ApplyError {
    fn from(error: SandboxStoreError) -> Self {
        Self::Domain(match error {
            SandboxStoreError::NotFound => Domain::NotFound,
            SandboxStoreError::Conflict => Domain::Conflict,
            SandboxStoreError::Corrupt(detail) => Domain::Corrupt(detail),
            other => Domain::Unavailable(other.to_string()),
        })
    }
}
impl From<CapacityError> for ApplyError {
    fn from(error: CapacityError) -> Self {
        match error {
            CapacityError::Domain(error) => error.into(),
            CapacityError::Driver(error) => error.into(),
        }
    }
}
pub(super) struct Snapshot {
    pub(super) agent: Agent,
    pub(super) worktree: Worktree,
    pub(super) workspace: Workspace,
    pub(super) sandbox: Sandbox,
}

pub(super) async fn load(
    store: &MongoSandboxStore,
    id: &AgentId,
    caller: &Caller,
    session: &mut ClientSession,
) -> Result<Snapshot, ApplyError> {
    let document = store
        .agents
        .find_one(doc! {"_id":id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(Domain::NotFound)?;
    let agent = read_agent(&document)?;
    if !caller.authorization.allows(Capability::ControlAnySandbox)
        && (caller.subject != agent.owner
            || !caller.authorization.allows(Capability::UseOwnSandboxes))
    {
        return Err(Domain::NotFound.into());
    }
    let worktree = store
        .worktrees
        .find_one(doc! {"_id":agent.worktree_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(Domain::WorktreeRecoveryConflict)?;
    let workspace = store
        .workspaces
        .find_one(doc! {"_id":agent.workspace_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(Domain::WorktreeRecoveryConflict)?;
    let sandbox = store
        .sandboxes
        .find_one(doc! {"_id":agent.sandbox_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(Domain::WorktreeRecoveryConflict)?;
    Ok(Snapshot {
        agent,
        worktree: read_worktree(&worktree)?,
        workspace: read_workspace(&workspace)?,
        sandbox: read_sandbox(&sandbox)?,
    })
}
fn filter(id: &str, revision: u64) -> Result<Document, ApplyError> {
    Ok(
        doc! {"_id":id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "revision": signed(revision,"worktree recovery revision").map_err(Domain::Corrupt)?,
        "channel_reuse_cleanup_execution":{"$exists":false}},
    )
}
pub(super) async fn fence_parents(
    store: &MongoSandboxStore,
    current: &Snapshot,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let mut parent = filter(current.sandbox.id.as_str(), current.sandbox.revision)?;
    parent.insert(
        "owned_workspace_cleanup_admission_v1",
        doc! {"$exists":false},
    );
    let result = store
        .sandboxes
        .update_one(parent, doc! {"$inc":{"workspace_admission_fence":1_i64}})
        .session(&mut *session)
        .await?;
    if result.matched_count != 1 {
        return Err(Domain::Conflict.into());
    }
    let mut parent = filter(current.workspace.id.as_str(), current.workspace.revision)?;
    parent.insert(
        "owned_workspace_cleanup_admission_v1",
        doc! {"$exists":false},
    );
    let result = store
        .workspaces
        .update_one(parent, doc! {"$inc":{"removal_lock_fence":1_i64}})
        .session(session)
        .await?;
    if result.matched_count != 1 {
        return Err(Domain::Conflict.into());
    }
    Ok(())
}
pub(super) async fn replace(
    store: &MongoSandboxStore,
    current: &Snapshot,
    next: &AgentWorktreeRecoveryMutation,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let mut agent = filter(current.agent.id.as_str(), current.agent.revision)?;
    let last_seq = signed(current.agent.conversation.last_seq, "agent log sequence")
        .map_err(Domain::Corrupt)?;
    agent.insert("last_seq", last_seq);
    agent.insert("record.conversation.last_seq", last_seq);
    agent.insert("channel_history_seal", doc! {"$exists":false});
    let result = store
        .agents
        .update_one(agent, doc! {"$set":agent_replacement_fields(&next.agent)?})
        .session(&mut *session)
        .await?;
    if result.matched_count != 1 {
        return Err(Domain::Conflict.into());
    }
    let mut worktree = filter(current.worktree.id.as_str(), current.worktree.revision)?;
    worktree.insert("record.agent_id", current.agent.id.as_str());
    let result = store
        .worktrees
        .update_one(
            worktree,
            doc! {"$set":worktree_replacement_fields(&next.retained_worktree)?},
        )
        .session(&mut *session)
        .await?;
    if result.matched_count != 1 {
        return Err(Domain::Conflict.into());
    }
    let mut workspace = filter(current.workspace.id.as_str(), current.workspace.revision)?;
    workspace.insert(
        "owned_workspace_cleanup_admission_v1",
        doc! {"$exists":false},
    );
    workspace.insert(
        "next_worktree_number",
        signed(current.workspace.next_worktree_number, "worktree number")
            .map_err(Domain::Corrupt)?,
    );
    let result = store
        .workspaces
        .update_one(
            workspace,
            doc! {"$set":workspace_replacement_fields(&next.workspace)?},
        )
        .session(&mut *session)
        .await?;
    if result.matched_count != 1 {
        return Err(Domain::Conflict.into());
    }
    store
        .worktrees
        .insert_one(worktree_document(&next.successor_worktree)?)
        .session(session)
        .await?;
    Ok(())
}
pub(super) async fn reconcile(
    store: &MongoSandboxStore,
    expected: &AgentWorktreeRecoveryMutation,
    timestamp: Timestamp,
) -> Result<Agent, Domain> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(timestamp)
        .await
        .map_err(|error| Domain::Unavailable(error.to_string()))?;
    let result: Result<Agent, ApplyError> = async {
        let document = store
            .agents
            .find_one(doc! {"_id":expected.agent.id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await?
            .ok_or(Domain::WorktreeRecoveryConflict)?;
        let current = read_agent(&document)?;
        let retained = store
            .worktrees
            .find_one(doc! {"_id":expected.retained_worktree.id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await?
            .ok_or(Domain::WorktreeRecoveryConflict)?;
        let successor = store
            .worktrees
            .find_one(doc! {"_id":expected.successor_worktree.id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await?
            .ok_or(Domain::WorktreeRecoveryConflict)?;
        let workspace = store
            .workspaces
            .find_one(doc! {"_id":expected.workspace.id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await?
            .ok_or(Domain::WorktreeRecoveryConflict)?;
        let workspace = read_workspace(&workspace)?;
        let sandbox = store
            .sandboxes
            .find_one(doc! {"_id":expected.agent.sandbox_id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await?
            .ok_or(Domain::WorktreeRecoveryConflict)?;
        let sandbox = read_sandbox(&sandbox)?;
        if workspace.id != expected.workspace.id
            || workspace.owner != expected.workspace.owner
            || workspace.sandbox_id != expected.workspace.sandbox_id
            || workspace.revision < expected.workspace.revision
            || workspace.next_worktree_number < expected.workspace.next_worktree_number
            || sandbox.id != expected.agent.sandbox_id
            || sandbox.owner != expected.agent.owner
        {
            return Err(Domain::WorktreeRecoveryConflict.into());
        }
        if !super::agent_control_transaction::same_agent_incarnation(&current, &expected.agent)
            || current.revision < expected.agent.revision
            || current.work.worktree_recoveries != expected.agent.work.worktree_recoveries
            || read_worktree(&retained)? != expected.retained_worktree
            || read_worktree(&successor)? != expected.successor_worktree
        {
            return Err(Domain::WorktreeRecoveryConflict.into());
        }
        Ok(current)
    }
    .await;
    result.map_err(|error| match error {
        ApplyError::Domain(Domain::Corrupt(detail)) => Domain::Corrupt(detail),
        _ => Domain::Unavailable("agent worktree recovery commit remains unknown".into()),
    })
}
