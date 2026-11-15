use super::*;

pub(super) struct GuardedSnapshot {
    pub record: SandboxHierarchyRecord,
    pub root: Document,
    pub policy: Option<Document>,
}

pub(super) async fn read(
    store: &ReadOnlyHierarchyStore,
    id: &SandboxId,
    session: &mut ClientSession,
) -> Result<Option<GuardedSnapshot>, SandboxStoreError> {
    let Some(root) = collection(&store.sandboxes, true)
        .find_one(doc! {"_id": id.as_str()})
        .max_time(DEADLINE)
        .session(&mut *session)
        .await
        .map_err(unavailable)?
    else {
        return Ok(None);
    };
    let sandbox = super::super::sandboxes::read_sandbox(&root)?;
    let policy = collection(&store.policies, true)
        .find_one(doc! {"_id": id.as_str()})
        .max_time(DEADLINE)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    if let Some(policy) = &policy {
        let personal = super::super::resource_access::read_inventory_policy(policy)
            .and_then(|policy| policy.personal_owner_of(&sandbox))
            .map_err(|_| closed())?;
        if !personal {
            return Ok(None);
        }
    }
    let workspaces = children(
        &store.workspaces,
        id,
        session,
        super::super::workspaces::read_workspace,
    )
    .await?;
    let worktrees = children(
        &store.worktrees,
        id,
        session,
        super::super::worktrees::read_worktree,
    )
    .await?;
    let agents = children(&store.agents, id, session, super::super::agents::read_agent).await?;
    Ok(Some(GuardedSnapshot {
        record: SandboxHierarchyRecord {
            sandbox,
            workspaces,
            worktrees,
            agents,
        },
        root,
        policy,
    }))
}

async fn children<T>(
    source: &Collection<Document>,
    id: &SandboxId,
    session: &mut ClientSession,
    parse: fn(&Document) -> Result<T, SandboxStoreError>,
) -> Result<Vec<T>, SandboxStoreError> {
    let mut cursor = collection(source, true)
        .find(doc! {"sandbox_id": id.as_str()})
        .sort(doc! {"_id": 1})
        .limit(10_001)
        .max_time(DEADLINE)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut values = Vec::new();
    while cursor.advance(&mut *session).await.map_err(unavailable)? {
        if values.len() == 10_000 {
            return Err(closed());
        }
        values.push(parse(&cursor.deserialize_current().map_err(unavailable)?)?);
    }
    Ok(values)
}
