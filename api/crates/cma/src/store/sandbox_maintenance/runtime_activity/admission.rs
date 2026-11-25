use layer2_core::sandbox::Agent;

use super::*;

/// Caller holds the process desktop exclusion through commit and unknown-ACK
/// reconciliation. These writes belong only to actual mutation admission.
pub(in crate::store::sandbox_maintenance) async fn validate_for_admission(
    store: &MongoSandboxStore,
    sandbox_document: &Document,
    agents: &[Agent],
    proof: &RuntimeActivityProof,
    authoritative_time_ms: i64,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    proof
        .validate_at(authoritative_time_ms)
        .map_err(ApplyError::Domain)?;
    let context = proof.source_context();
    if context.coverage != RuntimeActivityCoverage::MonotonicWritersV1
        || context.process_incarnation
            != layer2_core::sandbox::runtime_desktop_process_incarnation()
        || context.database_id != store.sandboxes.namespace().db
        || store.agent_ownership.is_some()
        || store.renewable_work
    {
        return Err(invalid());
    }
    let id = SandboxId(records::text(sandbox_document, "_id")?.into());
    let current = tokio::time::timeout(
        CAPTURE_BUDGET,
        capture::current(store, &id, context, session, false),
    )
    .await
    .map_err(|_| invalid())??;
    proof.validate_at(now_ms()?).map_err(ApplyError::Domain)?;
    if current.vector != proof.final_observation.vector
        || current.desktop != proof.final_observation.desktop
        || current.storage_incarnation != proof.final_observation.storage_incarnation
        || current.vector.has_activity()
        || current.desktop.opening_or_open != 0
    {
        return Err(invalid());
    }
    if current.vector.sandbox.revision != Some(records::number(sandbox_document, "revision")?)
        || current.vector.sandbox.last_seq != Some(records::number(sandbox_document, "last_seq")?)
        || agents.len() != current.vector.agents.len()
        || agents.iter().any(|agent| {
            !current.vector.agents.iter().any(|row| {
                row.id == agent.id.as_str()
                    && row.revision == Some(agent.revision)
                    && row.last_seq == Some(agent.conversation.last_seq)
            })
        })
    {
        return Err(invalid());
    }
    // Document writes conflict with every qualified parent/child insertion,
    // command and legacy/segmented log publication writer. No-op $set is not a
    // fence. The marker stays outside immutable aggregate/log-head documents.
    fence(&store.sandboxes, &current.vector.sandbox, session).await?;
    for (collection, rows) in [
        (&store.workspaces, &current.vector.workspaces),
        (&store.agents, &current.vector.agents),
        (&store.agent_command_orders, &current.vector.command_orders),
        (&store.agent_commands, &current.vector.commands),
    ] {
        for row in rows {
            fence(collection, row, session).await?;
        }
    }
    Ok(())
}

async fn fence(
    collection: &mongodb::Collection<Document>,
    row: &RuntimeActivityRecord,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    // Never let an exhausted counter promote to floating point, where adding
    // one could cease to modify the document and lose the conflict fence.
    let mut filter = doc! {"_id":&row.id, "$or":[
        {"runtime_migration_admission_fence":{"$exists":false}},
        {"runtime_migration_admission_fence":{"$type":"long", "$gte":0_i64, "$lt":i64::MAX}}
    ]};
    if let Some(revision) = row.revision {
        filter.insert("revision", i64::try_from(revision).map_err(|_| invalid())?);
    }
    if let Some(last_seq) = row.last_seq {
        filter.insert("last_seq", i64::try_from(last_seq).map_err(|_| invalid())?);
    }
    let updated = collection
        .update_one(
            filter,
            doc! {"$inc":{"runtime_migration_admission_fence":1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if updated.matched_count != 1 {
        return Err(invalid());
    }
    Ok(())
}
