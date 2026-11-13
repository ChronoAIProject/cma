//! Retain the exact child fences written by this checkpoint begin transaction.
use super::*;
use mongodb::bson::Document;

#[derive(Clone)]
struct Child {
    agent: Agent,
    source: Document,
    fence: i64,
}

#[derive(Clone)]
pub(in crate::store) struct Children {
    database: String,
    agents: Vec<Child>,
    workspaces: Vec<(layer2_core::sandbox::Workspace, i64)>,
}

pub(super) async fn capture(
    store: &MongoSandboxStore,
    current: &Sandbox,
    next: &Sandbox,
    agents: &[Agent],
    scope: &mut TransactionScope<'_>,
) -> Result<Option<Children>, ApplyError> {
    if current.lifecycle.checkpoint_capture.is_some() || next.lifecycle.checkpoint_capture.is_none()
    {
        return Ok(None);
    }
    let mut children = Vec::with_capacity(agents.len());
    for agent in agents {
        let row = scope
            .find_one(&store.agents, doc! {"_id":agent.id.as_str()})
            .await
            .map_err(ApplyError::Driver)?
            .ok_or_else(refused)?;
        if read_agent(&row).map_err(ApplyError::Domain)? != *agent {
            return Err(refused());
        }
        let fence = row
            .get_i64("checkpoint_capture_fence")
            .map_err(|_| refused())?;
        if fence < 1 {
            return Err(refused());
        }
        children.push(Child {
            agent: agent.clone(),
            source: binding(&row).map_err(ApplyError::Domain)?,
            fence,
        });
    }
    let workspaces =
        crate::store::agent_capacity_recovery::lineage::workspaces(store, &current.id, scope)
            .await?;
    let mut workspace_fences = Vec::with_capacity(workspaces.len());
    for workspace in workspaces {
        let row = scope
            .find_one(&store.workspaces, doc! {"_id":workspace.id.as_str()})
            .await
            .map_err(ApplyError::Driver)?
            .ok_or_else(refused)?;
        let fence = row
            .get_i64("checkpoint_capture_fence")
            .map_err(|_| refused())?;
        if fence < 1 {
            return Err(refused());
        }
        workspace_fences.push((workspace, fence));
    }
    Ok(Some(Children {
        database: store.agents.namespace().db,
        agents: children,
        workspaces: workspace_fences,
    }))
}

pub(super) async fn matches(
    store: &MongoSandboxStore,
    children: &Option<Children>,
    session: &mut mongodb::ClientSession,
) -> Result<bool, SandboxStoreError> {
    let Some(children) = children else {
        return Ok(true);
    };
    if children.database != store.agents.namespace().db {
        return Ok(false);
    }
    for child in &children.agents {
        let row = store
            .agents
            .find_one(doc! {"_id":child.agent.id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let Some(row) = row else {
            return Ok(false);
        };
        if read_agent(&row)? != child.agent
            || binding(&row)? != child.source
            || row.get_i64("checkpoint_capture_fence").ok() != Some(child.fence)
        {
            return Ok(false);
        }
    }
    for (workspace, fence) in &children.workspaces {
        let row = store
            .workspaces
            .find_one(doc! {"_id":workspace.id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let Some(row) = row else {
            return Ok(false);
        };
        if crate::store::workspaces::read_workspace(&row)? != *workspace
            || row.get_i64("checkpoint_capture_fence").ok() != Some(*fence)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn binding(row: &Document) -> Result<Document, SandboxStoreError> {
    // Reject malformed/unknown envelopes, including partial segmented fields.
    crate::store::agent_log_segments::read_format(row)?;
    let mut source = Document::new();
    for field in [
        "document_schema_version",
        "agent_log_format",
        "agent_log_incarnation",
        "agent_log_head_revision",
    ] {
        if let Some(value) = row.get(field) {
            source.insert(field, value.clone());
        }
    }
    Ok(source)
}

fn refused() -> ApplyError {
    ApplyError::Domain(SandboxStoreError::Conflict)
}
