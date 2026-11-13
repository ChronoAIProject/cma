//! Stream individually validated children, never one joined hierarchy BSON row.
use super::agent_projection::AgentReader;
use super::*;
use layer2_core::sandbox::{Workspace, Worktree};

// Leave more than the fixed envelope overhead under Mongo's per-document limit.
const ENVELOPE_SAFE_BYTES: i32 = 16 * 1024 * 1024 - 128;

pub(super) struct Children<A> {
    pub workspaces: Vec<Workspace>,
    pub worktrees: Vec<Worktree>,
    pub agents: Vec<A>,
}

fn branch(id: &SandboxId, kind: &str) -> Vec<Document> {
    vec![
        doc! {"$match": {"sandbox_id": id.as_str()}},
        doc! {"$replaceWith": {"$cond": [
            {"$gt": [{"$bsonSize": "$$ROOT"}, ENVELOPE_SAFE_BYTES]},
            {"kind": {"$literal": "oversized"}},
            {"kind": {"$literal": kind}, "value": "$$ROOT"},
        ]}},
    ]
}

/// None requests the same-snapshot compatibility read for a near-limit record.
/// No driver or decoder failure is converted into a fallback or partial result.
pub(super) async fn read<R: AgentReader>(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
) -> Result<Option<Children<R::Record>>, Error> {
    let mut pipeline = branch(id, "workspace");
    pipeline.push(
        doc! {"$unionWith": {"coll": store.worktrees.name(), "pipeline": branch(id, "worktree")}},
    );
    let mut agents = branch(id, "agent");
    if let Some(projection) = R::projection() {
        // Project before the envelope-size check: the large unused payload must
        // neither cross the wire nor force a full-Agent fallback.
        agents.insert(1, doc! {"$project": projection});
    }
    pipeline.push(doc! {"$unionWith": {"coll": store.agents.name(), "pipeline": agents}});
    let mut cursor = collection(&store.workspaces, true)
        .aggregate(pipeline)
        .max_time(std::time::Duration::from_secs(10))
        .session(&mut *session)
        .await?;
    let mut children = Children {
        workspaces: Vec::new(),
        worktrees: Vec::new(),
        agents: Vec::new(),
    };
    while cursor.advance(&mut *session).await? {
        let row = cursor.deserialize_current()?;
        let kind = row.get_str("kind").map_err(|_| invalid_row())?;
        if kind == "oversized" {
            return Ok(None);
        }
        let value = row.get_document("value").map_err(|_| invalid_row())?;
        match kind {
            "workspace" => children.workspaces.push(read_workspace(value)?),
            "worktree" => children.worktrees.push(read_worktree(value)?),
            "agent" => children.agents.push(R::read(value)?),
            _ => return Err(invalid_row().into()),
        }
    }
    // $unionWith does not promise output ordering. Preserve the original
    // per-collection _id order without a blocking server sort or joined array.
    children
        .workspaces
        .sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    children
        .worktrees
        .sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    children.agents.sort_by(|a, b| R::id(a).cmp(R::id(b)));
    Ok(Some(children))
}

fn invalid_row() -> SandboxStoreError {
    SandboxStoreError::Corrupt("invalid tagged hierarchy row".into())
}
