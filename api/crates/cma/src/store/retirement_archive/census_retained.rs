//! Recover lineage from retained authoritative records, including removed Agents.
use super::*;
pub(super) async fn extend(
    db: &Database,
    sandbox: &str,
    session: &mut ClientSession,
    ids: &mut BTreeSet<String>,
    agents: &mut BTreeSet<String>,
) -> Result<()> {
    // Provenance outlives deleted resources. Resolve those Agent IDs explicitly;
    // never let disappearance of the live aggregate hide retained child history.
    let mut provenance = db
        .collection::<Document>("agent_creation_provenance")
        .find(doc! {"record.sandbox_id":sandbox})
        .session(&mut *session)
        .await
        .map_err(|_| REFUSED)?;
    while provenance
        .advance(&mut *session)
        .await
        .map_err(|_| REFUSED)?
    {
        let row = provenance.deserialize_current().map_err(|_| REFUSED)?;
        let value = crate::store::agent_provenance::read(&row).map_err(|_| REFUSED)?;
        if value.sandbox_id.as_str() != sandbox {
            return Err(REFUSED);
        }
        agents.insert(value.agent_id.to_string());
        ids.extend([
            value.agent_id.to_string(),
            value.workspace_id.to_string(),
            value.worktree_id.to_string(),
        ]);
        if ids.len() > 100_000 {
            return Err(REFUSED);
        }
    }

    let mut requests = db
        .collection::<Document>("agent_access_requests")
        .find(doc! {"record.requester.sandbox_id":sandbox})
        .session(&mut *session)
        .await
        .map_err(|_| REFUSED)?;
    while requests.advance(&mut *session).await.map_err(|_| REFUSED)? {
        let row = requests.deserialize_current().map_err(|_| REFUSED)?;
        let value = crate::store::agent_access_requests::read(&row).map_err(|_| REFUSED)?;
        agents.insert(value.requester.agent_id.to_string());
        ids.extend([
            value.requester.agent_id.to_string(),
            value.requester.creation.workspace_id.to_string(),
            value.requester.creation.worktree_id.to_string(),
        ]);
        if ids.len() > 100_000 {
            return Err(REFUSED);
        }
    }
    for name in ["agent_lifecycle_operations", "agent_reuse_bindings"] {
        let mut cursor = db
            .collection::<Document>(name)
            .find(doc! {"sandbox_id":sandbox})
            .session(&mut *session)
            .await
            .map_err(|_| REFUSED)?;
        while cursor.advance(&mut *session).await.map_err(|_| REFUSED)? {
            let row = cursor.deserialize_current().map_err(|_| REFUSED)?;
            if let Some(Bson::String(agent)) = row.get("agent_id") {
                agents.insert(agent.clone());
                ids.insert(agent.clone());
            } else if name == "agent_reuse_bindings"
                || !matches!(row.get("agent_id"), None | Some(Bson::Null))
            {
                return Err(REFUSED);
            }
            if ids.len() > 100_000 {
                return Err(REFUSED);
            }
        }
    }
    // Older removed Agents may retain their creation log but no provenance row.
    // A typed Created entry binds them to the retained Workspace/Worktree closure.
    let resources: Vec<_> = ids.iter().cloned().collect();
    let mut logs = db
        .collection::<Document>("agent_log")
        .find(doc! {
            "record.kind.type":"created", "$or":[
                {"record.kind.workspace_id":{"$in":&resources}},
                {"record.kind.worktree_id":{"$in":&resources}}
            ]
        })
        .session(&mut *session)
        .await
        .map_err(|_| REFUSED)?;
    while logs.advance(&mut *session).await.map_err(|_| REFUSED)? {
        let row = logs.deserialize_current().map_err(|_| REFUSED)?;
        let entry = crate::store::agent_log::read_entry(&row).map_err(|_| REFUSED)?;
        if let layer2_core::sandbox::AgentLogKind::Created {
            workspace_id,
            worktree_id,
        } = entry.kind
        {
            if entry.seq != 1
                || !ids.contains(workspace_id.as_str())
                || !ids.contains(worktree_id.as_str())
            {
                return Err(REFUSED);
            }
            agents.insert(entry.agent_id.to_string());
            ids.insert(entry.agent_id.to_string());
        } else {
            return Err(REFUSED);
        }
        if ids.len() > 100_000 {
            return Err(REFUSED);
        }
    }
    // Never let stale/contradictory provenance sweep another extant parent in.
    let agent_ids: Vec<_> = agents.iter().cloned().collect();
    if db.collection::<Document>("agents").find_one(doc! {
        "_id":{"$in":agent_ids},"$or":[{"sandbox_id":{"$ne":sandbox}},{"record.sandbox_id":{"$ne":sandbox}}]
    }).session(&mut *session).await.map_err(|_| REFUSED)?.is_some() { return Err(REFUSED); }
    Ok(())
}
