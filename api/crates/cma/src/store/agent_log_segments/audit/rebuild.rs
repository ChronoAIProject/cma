//! Publish only a fully validated generation under exact Agent/head CAS.
use super::super::{projection, PROJECTIONS};
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(crate) async fn install(
    client: &Client,
    database: &Database,
    agent: &str,
    audit: &str,
) -> Result<String, SandboxStoreError> {
    let mut session = client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let result = install_transaction(database, agent, audit, &mut session).await;
    match result {
        Ok(status) => {
            commit(&mut session).await?;
            Ok(status)
        }
        Err(error) => {
            let _ = session.abort_guarded().await;
            Err(error)
        }
    }
}
async fn install_transaction(
    database: &Database,
    agent_id: &str,
    audit: &str,
    session: &mut ClientSession,
) -> Result<String, SandboxStoreError> {
    let audits = collection(database, AUDITS);
    let row = audits
        .find_one(doc! {"_id":id(agent_id,audit)})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    let mut progress = progress::read(row, agent_id, audit)?;
    if progress.status == "installed" {
        return Ok(progress.status);
    }
    if progress.status != "complete" || progress.expires_at_ms != 0 || progress.stage.is_some() {
        return Err(SandboxStoreError::Conflict);
    }
    let (agent, current) = progress::current(database, agent_id, session, false).await?;
    if agent != progress.captured_agent || current != progress.captured_head {
        return Err(SandboxStoreError::Conflict);
    }
    let complete = &progress.completed;
    if complete.checked_entries != current.last_seq
        || complete.checked_bytes != current.lifetime_bytes
        || complete.next_ordinal != current.active_ordinal + 1
    {
        return Err(corrupt());
    }
    complete
        .semantic
        .validate_aggregate(&agent)
        .map_err(SandboxStoreError::Corrupt)?;
    let mut replacement = progress.head(&progress.result_generation);
    replacement.revision = current.revision.checked_add(1).ok_or_else(corrupt)?;
    replacement.validate()?;
    let projection = bson::serialize_to_document(&projection::Projection {
        agent_id: agent_id.into(),
        format_version: 1,
        incarnation: current.incarnation.clone(),
        evidence_generation: replacement.evidence_generation.clone(),
        through_seq: current.last_seq,
        head_revision: replacement.revision,
        record: complete.semantic.clone(),
    })
    .map_err(|_| corrupt())?;
    if bson::serialize_to_vec(&projection)
        .map_err(|_| corrupt())?
        .len()
        > 256 * 1024
    {
        return Err(corrupt());
    }
    if collection(database, HEADS)
        .replace_one(
            doc! {"_id":agent_id,"incarnation":&current.incarnation,"revision":current.revision},
            bson::serialize_to_document(&replacement).map_err(|_| corrupt())?,
        )
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .matched_count
        != 1
    {
        return Err(SandboxStoreError::Conflict);
    }
    if collection(database,"agents").update_one(
        doc! {"_id":agent_id,"document_schema_version":2_i64,"agent_log_incarnation":&current.incarnation,
            "agent_log_head_revision":current.revision,"revision":agent.revision as i64},
        doc! {"$set":{"agent_log_head_revision":replacement.revision}},
    ).session(&mut *session).await.map_err(unavailable)?.matched_count!=1 {return Err(SandboxStoreError::Conflict);}
    collection(database, PROJECTIONS)
        .replace_one(doc! {"_id":agent_id}, projection)
        .upsert(true)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    progress.status = "installed".into();
    if audits.replace_one(doc! {"_id":&progress.id,"status":"complete","fence":progress.fence,"step":progress.step},progress.document()?)
        .session(session).await.map_err(unavailable)?.matched_count!=1 {return Err(SandboxStoreError::Conflict);}
    Ok(progress.status)
}
