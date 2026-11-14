use super::{agent_projection::AgentReader, *};

pub(super) async fn read<R: AgentReader>(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
) -> Result<Option<SandboxHierarchyRecord<R::Record>>, Error> {
    let snapshot = store.sandbox_snapshot_reads;
    let sandboxes = collection(&store.sandboxes, snapshot);
    let Some(document) = sandboxes
        .find_one(doc! {"_id":id.as_str()})
        .max_time(std::time::Duration::from_secs(10))
        .session(&mut *session)
        .await?
    else {
        return Ok(None);
    };
    let sandbox = read_sandbox(&document)?;
    if snapshot && store.sandbox_batched_reads {
        if let Some(children) = batched::read::<R>(store, id, session).await? {
            return Ok(Some(SandboxHierarchyRecord {
                sandbox,
                workspaces: children.workspaces,
                worktrees: children.worktrees,
                agents: children.agents,
            }));
        }
        // A selected row too large for its envelope needs compatibility reads.
        // Keep the same session so the fallback cannot mix snapshot times.
    }
    let workspaces = children(
        &collection(&store.workspaces, snapshot),
        doc! {"sandbox_id":id.as_str()},
        session,
        read_workspace,
    )
    .await?;
    let worktrees = children(
        &collection(&store.worktrees, snapshot),
        doc! {"sandbox_id":id.as_str()},
        session,
        read_worktree,
    )
    .await?;
    let agents = collection(&store.agents, snapshot);
    let mut find = agents
        .find(doc! {"sandbox_id":id.as_str()})
        .sort(doc! {"_id":1})
        .max_time(std::time::Duration::from_secs(10));
    if let Some(projection) = R::projection() {
        find = find.projection(projection);
    }
    let mut cursor = find.session(&mut *session).await?;
    let mut agents = Vec::new();
    while cursor.advance(&mut *session).await? {
        agents.push(R::read(&cursor.deserialize_current()?)?);
    }
    Ok(Some(SandboxHierarchyRecord {
        sandbox,
        workspaces,
        worktrees,
        agents,
    }))
}
