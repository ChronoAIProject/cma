use layer2_core::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStoreError, ResourceLineage, ResourceTarget,
};
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use super::super::{
    agents::read_agent, sandbox_mongo::MongoSandboxStore, sandboxes::read_sandbox,
    workspaces::read_workspace, worktrees::read_worktree,
};
use super::{records, ReadError};

pub(super) async fn read(
    store: &MongoSandboxStore,
    target: &ResourceTarget,
    session: &mut ClientSession,
) -> Result<ResourceAccessSnapshot, ReadError> {
    let agent = if let ResourceTarget::Agent { agent_id } = target {
        Some(
            read_agent(&get(&store.agents, agent_id.as_str(), true, session).await?)
                .map_err(|_| ResourceAccessStoreError::Corrupt)?,
        )
    } else {
        None
    };
    let worktree_id = match target {
        ResourceTarget::Worktree { worktree_id } => Some(worktree_id),
        _ => agent.as_ref().map(|agent| &agent.worktree_id),
    };
    let worktree = if let Some(id) = worktree_id {
        Some(
            read_worktree(
                &get(
                    &store.worktrees,
                    id.as_str(),
                    matches!(target, ResourceTarget::Worktree { .. }),
                    session,
                )
                .await?,
            )
            .map_err(|_| ResourceAccessStoreError::Corrupt)?,
        )
    } else {
        None
    };
    let workspace_id = match target {
        ResourceTarget::Workspace { workspace_id } => Some(workspace_id),
        _ => worktree.as_ref().map(|worktree| &worktree.workspace_id),
    };
    let workspace = if let Some(id) = workspace_id {
        Some(
            read_workspace(
                &get(
                    &store.workspaces,
                    id.as_str(),
                    matches!(target, ResourceTarget::Workspace { .. }),
                    session,
                )
                .await?,
            )
            .map_err(|_| ResourceAccessStoreError::Corrupt)?,
        )
    } else {
        None
    };
    let sandbox_id = match target {
        ResourceTarget::Sandbox { sandbox_id } => sandbox_id,
        _ => {
            &workspace
                .as_ref()
                .ok_or(ResourceAccessStoreError::Corrupt)?
                .sandbox_id
        }
    };
    let sandbox = read_sandbox(
        &get(
            &store.sandboxes,
            sandbox_id.as_str(),
            matches!(target, ResourceTarget::Sandbox { .. }),
            session,
        )
        .await?,
    )
    .map_err(|_| ResourceAccessStoreError::Corrupt)?;
    let sandbox_id = sandbox.id.clone();
    let lineage = ResourceLineage {
        sandbox,
        workspace,
        worktree,
        agent,
    };
    lineage.validate(target)?;
    if matches!(target, ResourceTarget::Worktree { .. }) {
        if let Some(id) = lineage
            .worktree
            .as_ref()
            .and_then(|tree| tree.agent_id.as_ref())
        {
            let assigned = read_agent(&get(&store.agents, id.as_str(), false, session).await?)
                .map_err(|_| ResourceAccessStoreError::Corrupt)?;
            lineage.validate_worktree_agent(&assigned)?;
        }
    }
    let policy = store
        .resource_policies
        .find_one(doc! {"_id":sandbox_id.as_str()})
        .session(&mut *session)
        .await?
        .as_ref()
        .map(records::read_policy)
        .transpose()?;
    let mut selectors = vec![doc! {"selector_kind":"sandbox","selector_id":sandbox_id.as_str()}];
    if let Some(agent) = &lineage.agent {
        selectors.push(doc! {"selector_kind":"agent","selector_id":agent.id.as_str()});
    }
    let mut cursor = store
        .resource_shares
        .find(doc! {"sandbox_id":sandbox_id.as_str(),"revoked":false,"$or":selectors})
        .sort(doc! {"_id": 1})
        .limit(layer2_core::resource_access::MAX_ACTIVE_RESOURCE_SHARES as i64 + 1)
        .session(&mut *session)
        .await?;
    let mut shares = Vec::new();
    while cursor.advance(&mut *session).await? {
        let document = cursor.deserialize_current()?;
        shares.push(records::read_share(&sandbox_id, &document)?);
        if shares.len() > layer2_core::resource_access::MAX_ACTIVE_RESOURCE_SHARES {
            return Err(ResourceAccessStoreError::Capacity.into());
        }
    }
    let snapshot = ResourceAccessSnapshot {
        lineage,
        policy,
        shares,
    };
    snapshot.scope(target)?;
    Ok(snapshot)
}
async fn get(
    collection: &Collection<Document>,
    id: &str,
    is_target: bool,
    session: &mut ClientSession,
) -> Result<Document, ReadError> {
    collection
        .find_one(doc! {"_id":id})
        .session(session)
        .await?
        .ok_or_else(|| {
            if is_target {
                ResourceAccessStoreError::NotFound.into()
            } else {
                ResourceAccessStoreError::Corrupt.into()
            }
        })
}
