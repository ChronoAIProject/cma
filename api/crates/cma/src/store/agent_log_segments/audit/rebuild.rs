//! Rebuild into an isolated generation, then publish one pointer under the
//! captured Agent/head CAS. Incomplete or stale rebuilds cannot become live.
use super::*;
use crate::store::agent_log_segments::{projection, EVIDENCE, PROJECTIONS};
use layer2_core::sandbox::{AgentLogKind, QueuedMessageState};
use sha2::{Digest, Sha256};

pub(super) fn head(progress: &Progress) -> Head {
    let mut head = progress.captured_head.clone();
    head.evidence_generation = format!("audit:{}", progress.audit_id);
    head
}

/// The aggregate queue is bounded. Check all retained queue records against
/// their original source, including terminal/delivered records; a terminal
/// classification is not evidence that its input never executed.
pub(super) async fn validate_queue(
    database: &Database,
    progress: &Progress,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    let rebuilt = head(progress);
    for queued in &progress.captured_agent.conversation.queued_messages {
        let key = queued.sequence.to_string();
        let document = database
            .collection::<Document>(EVIDENCE)
            .find_one(doc! {"_id":projection::identity(&rebuilt, "queued", &key)})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?;
        let record = projection::read_evidence(&rebuilt, "queued", &key, document)?;
        if !matches!(record, projection::EvidenceRecord::Queued { sequence, consumed_by }
            if sequence == queued.sequence && (queued.state != QueuedMessageState::Queued || consumed_by.is_none()))
        {
            return Err(corrupt());
        }
        let source = database.collection::<Document>("agent_log")
            .find_one(doc! {"agent_id":&progress.agent_id,"seq":i64::try_from(queued.sequence).map_err(|_|corrupt())?})
            .session(&mut *session).await.map_err(unavailable)?.ok_or_else(corrupt)?;
        let entry = read_entry(&source)?;
        if entry.agent_id.as_str() != progress.agent_id || entry.seq != queued.sequence {
            return Err(corrupt());
        }
        let AgentLogKind::OwnerMessage { content } = entry.kind else {
            return Err(corrupt());
        };
        if format!("{:x}", Sha256::digest(content.as_bytes())) != queued.payload_sha256 {
            return Err(corrupt());
        }
    }
    Ok(())
}

pub(crate) async fn install(
    client: &Client,
    database: &Database,
    agent_id: &str,
    audit_id: &str,
) -> Result<String, SandboxStoreError> {
    let mut session = client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let result = install_transaction(database, agent_id, audit_id, &mut session).await;
    match result {
        Ok(status) => {
            commit_unit(&mut session).await?;
            Ok(status)
        }
        Err(error) => {
            let _ = session.abort_transaction().await;
            Err(error)
        }
    }
}

async fn install_transaction(
    database: &Database,
    agent_id: &str,
    audit_id: &str,
    session: &mut ClientSession,
) -> Result<String, SandboxStoreError> {
    let id = format!("{}:{agent_id}{}:{audit_id}", agent_id.len(), audit_id.len());
    let collection = database.collection::<Document>(AUDITS);
    let document = collection
        .find_one(doc! {"_id":&id})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    let mut progress: Progress =
        bson::deserialize_from_document(document).map_err(|_| corrupt())?;
    if progress.agent_id != agent_id
        || progress.audit_id != audit_id
        || progress.id != id
        || progress.format_version != 1
    {
        return Err(corrupt());
    }
    if progress.status == "installed" {
        // Immutable installation outcome, including after subsequent appends.
        return Ok(progress.status);
    }
    if progress.status != "complete" || progress.expires_at_ms != 0 {
        return Err(SandboxStoreError::Conflict);
    }
    let (agent, current) = progress::current(database, agent_id, session).await?;
    if current != progress.captured_head || agent != progress.captured_agent {
        return Err(SandboxStoreError::Conflict);
    }
    if progress.checked_entries != current.last_seq
        || progress.checked_bytes != current.lifetime_bytes
        || progress.next_ordinal != current.active_ordinal + 1
    {
        return Err(corrupt());
    }
    progress
        .semantic
        .validate_aggregate(&agent)
        .map_err(SandboxStoreError::Corrupt)?;
    let mut replacement = head(&progress);
    replacement.revision = current.revision.checked_add(1).ok_or_else(corrupt)?;
    replacement.validate()?;
    let result = database
        .collection::<Document>(HEADS)
        .replace_one(
            doc! {"_id":agent_id,"revision":current.revision},
            bson::serialize_to_document(&replacement).map_err(|_| corrupt())?,
        )
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    let result = database
        .collection::<Document>("agents")
        .update_one(
            doc! {"_id":agent_id,"agent_log_head_revision":current.revision},
            doc! {"$set":{"agent_log_head_revision":replacement.revision}},
        )
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    let projection = projection::Projection {
        agent_id: agent_id.into(),
        format_version: 1,
        incarnation: current.incarnation.clone(),
        evidence_generation: replacement.evidence_generation,
        through_seq: current.last_seq,
        head_revision: replacement.revision,
        record: progress.semantic.clone(),
    };
    database
        .collection::<Document>(PROJECTIONS)
        .replace_one(
            doc! {"_id":agent_id},
            bson::serialize_to_document(&projection).map_err(|_| corrupt())?,
        )
        .upsert(true)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    progress.status = "installed".into();
    let result = collection
        .replace_one(
            doc! {"_id":&id,"status":"complete","fence":progress.fence},
            bson::serialize_to_document(&progress).map_err(|_| corrupt())?,
        )
        .session(session)
        .await
        .map_err(unavailable)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(progress.status)
}
