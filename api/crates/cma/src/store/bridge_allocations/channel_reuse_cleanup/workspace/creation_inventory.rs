//! Creation receipts survive lifecycle retirement and cannot hide missing children.
use super::*;
pub(super) async fn holds(
    database: &Database,
    sandbox: &Sandbox,
    child: &ChannelReuseRemovalState,
    session: &mut ClientSession,
) -> Result<Vec<WorkspaceCleanupHold>, JobError> {
    let target = &child.seal.target;
    let mut holds = Vec::new();
    let mut provenance = majority_collection::<Document>(database, "agent_creation_provenance")
        .find(doc! {"$or":[{"_id":target.agent_id.as_str()},{"record.agent_id":target.agent_id.as_str()},
            {"record.workspace_id":target.workspace_id.as_str()},{"record.worktree_id":target.worktree_id.as_str()}]})
        .limit(257).session(&mut *session).await?;
    let mut count = 0;
    while provenance.advance(&mut *session).await? {
        count += 1;
        if count > 256 {
            holds.push(WorkspaceCleanupHold::InventoryLimit);
            break;
        }
        let row: Document = provenance.deserialize_current()?;
        let provenance =
            crate::store::agent_provenance::read(&row).map_err(|_| CleanupError::Corrupt)?;
        let agent_row = read(database, "agents", provenance.agent_id.as_str(), session).await?;
        let agent =
            crate::store::agents::read_agent(&agent_row).map_err(|_| CleanupError::Corrupt)?;
        provenance
            .validate_against(&agent, sandbox)
            .map_err(|_| CleanupError::Corrupt)?;
        if provenance.workspace_id != target.workspace_id || provenance.owner != child.seal.owner {
            return Err(CleanupError::Corrupt.into());
        }
        if provenance.agent_id == target.agent_id {
            if digest(&provenance)? != child.seal.creation_sha256 {
                return Err(CleanupError::Corrupt.into());
            }
        } else {
            holds.push(WorkspaceCleanupHold::OtherWorkspaceChildren);
        }
        fence_row(database, "agents", agent_row, session).await?;
        fence_row(database, "agent_creation_provenance", row, session).await?;
    }
    if count == 0 {
        return Err(CleanupError::Corrupt.into());
    }
    // Bindings contain a parent and Agent, but no Workspace. Resolve the
    // bounded parent inventory; a missing target cannot safely mean unrelated.
    let mut bindings = majority_collection::<Document>(database, "agent_reuse_bindings")
        .find(
            doc! {"$or":[{"sandbox_id":sandbox.id.as_str()},{"agent_id":target.agent_id.as_str()},
            {"creation_key":format!("channel-allocation:{}",child.seal.allocation_id)}]},
        )
        .limit(257)
        .session(&mut *session)
        .await?;
    let mut count = 0;
    while bindings.advance(&mut *session).await? {
        count += 1;
        if count > 256 {
            holds.push(WorkspaceCleanupHold::InventoryLimit);
            break;
        }
        let row: Document = bindings.deserialize_current()?;
        let agent_id = row.get_str("agent_id").map_err(|_| CleanupError::Corrupt)?;
        let key = row
            .get_str("creation_key")
            .map_err(|_| CleanupError::Corrupt)?;
        let fingerprint = row
            .get_str("fingerprint")
            .map_err(|_| CleanupError::Corrupt)?;
        let agent_row = read(database, "agents", agent_id, session).await?;
        let agent =
            crate::store::agents::read_agent(&agent_row).map_err(|_| CleanupError::Corrupt)?;
        if row.get_i64("document_schema_version").ok() != Some(DOCUMENT_SCHEMA_VERSION)
            || row.get_str("_id").ok() != Some(digest(&(agent.owner.as_str(), key))?.as_str())
            || row.get_str("owner").ok() != Some(agent.owner.as_str())
            || row.get_str("sandbox_id").ok() != Some(sandbox.id.as_str())
            || agent.sandbox_id != sandbox.id
            || agent.owner != sandbox.owner
            || key.is_empty()
            || fingerprint.len() != 64
            || !fingerprint
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(CleanupError::Corrupt.into());
        }
        if agent.workspace_id == target.workspace_id && agent.id != target.agent_id {
            holds.push(WorkspaceCleanupHold::OtherWorkspaceChildren);
        }
        fence_row(database, "agents", agent_row, session).await?;
        fence_row(database, "agent_reuse_bindings", row, session).await?;
    }
    Ok(holds)
}
