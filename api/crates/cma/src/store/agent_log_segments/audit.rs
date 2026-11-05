//! One resumable, fenced segment scrub per invocation. No lifetime snapshot.
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{
    bson::{self, doc, Binary, Document, RawDocumentBuf},
    Client, ClientSession, Database,
};
use serde::{Deserialize, Serialize};

use super::{
    corrupt, digest, read_format, Head, Seal, AUDITS, HEADS, MAX_BYTES, MAX_ENTRIES, SEGMENTS,
};
use crate::store::{
    agent_log::read_entry,
    agents::read_agent,
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    #[serde(rename = "_id")]
    id: String,
    format_version: i64,
    agent_id: String,
    incarnation: String,
    audit_id: String,
    holder: String,
    expires_at_ms: i64,
    fence: i64,
    status: String,
    through_seq: i64,
    head_revision: i64,
    next_ordinal: i64,
    checked_entries: i64,
    checked_bytes: i64,
    #[serde(deserialize_with = "super::required_nullable")]
    previous_seal: Option<Binary>,
    captured_head: Head,
    captured_agent: layer2_core::sandbox::Agent,
    semantic: layer2_core::sandbox::AgentLogProjectionState,
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(format!("Agent log audit storage: {error}"))
}

mod progress;
mod rebuild;
pub(in crate::store) use rebuild::install;

/// Claim a visible fenced lease before scanning one bounded unit. A process
/// crash leaves a lease that another invocation can take after expiry, while
/// its stale transaction cannot publish progress under the successor fence.
pub(crate) async fn run_unit(
    client: &Client,
    database: &Database,
    agent_id: &str,
    audit_id: &str,
    cancel: bool,
) -> Result<String, SandboxStoreError> {
    if agent_id.is_empty() || agent_id.len() > 512 || audit_id.is_empty() || audit_id.len() > 128 {
        return Err(SandboxStoreError::InvalidPage);
    }
    let mut session = client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let claimed = match progress::claim(database, agent_id, audit_id, cancel, &mut session).await {
        Ok(progress) => progress,
        Err(error) => {
            let _ = session.abort_transaction().await;
            return Err(error);
        }
    };
    commit_unit(&mut session).await?;
    if claimed.status != "running" {
        return Ok(claimed.status);
    }
    start_transaction(&mut session).await.map_err(unavailable)?;
    let result = finish(database, claimed, &mut session).await;
    let status = match result {
        Ok(status) => status,
        Err(error) => {
            let _ = session.abort_transaction().await;
            return Err(error);
        }
    };
    commit_unit(&mut session).await?;
    Ok(status)
}

async fn commit_unit(session: &mut ClientSession) -> Result<(), SandboxStoreError> {
    match commit_transaction(session).await {
        CommitOutcome::Committed => Ok(()),
        _ => Err(SandboxStoreError::Unavailable(
            "Agent log audit commit unresolved; retry the same audit ID".into(),
        )),
    }
}

async fn finish(
    database: &Database,
    mut progress: Progress,
    session: &mut ClientSession,
) -> Result<String, SandboxStoreError> {
    let now = bson::DateTime::now().timestamp_millis();
    let collection = database.collection::<Document>(AUDITS);
    let filter = |now| doc! {"_id":&progress.id,"holder":&progress.holder,"fence":progress.fence,"status":"running","expires_at_ms":{"$gt":now}};
    if collection
        .find_one(filter(now))
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .is_none()
    {
        return Err(SandboxStoreError::Conflict);
    }
    let (agent, current) = progress::current(database, &progress.agent_id, session).await?;
    let before = progress.clone();
    match scrub(database, &agent, &current, &mut progress, session).await {
        Ok(()) => {}
        Err(SandboxStoreError::Corrupt(_) | SandboxStoreError::SequenceConflict) => {
            progress = before;
            progress.status = "corrupt".into();
            tracing::error!(
                audit_outcome = "corrupt",
                "Agent log audit quarantined a retained history"
            );
        }
        Err(error) => return Err(error),
    }
    let now = bson::DateTime::now().timestamp_millis();
    if now >= progress.expires_at_ms {
        return Err(SandboxStoreError::Conflict);
    }
    let filter = doc! {"_id":&progress.id,"holder":&progress.holder,"fence":progress.fence,"status":"running","expires_at_ms":{"$gt":now}};
    progress.expires_at_ms = 0;
    let row = bson::serialize_to_document(&progress).map_err(|_| corrupt())?;
    let result = collection
        .replace_one(filter, row)
        .session(session)
        .await
        .map_err(unavailable)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(progress.status)
}

async fn scrub(
    database: &Database,
    agent: &layer2_core::sandbox::Agent,
    current: &Head,
    progress: &mut Progress,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    let captured = &progress.captured_head;
    if current.agent_id != captured.agent_id
        || current.incarnation != captured.incarnation
        || current.revision < captured.revision
        || current.last_seq < captured.last_seq
        || current.active_ordinal < captured.active_ordinal
    {
        return Err(corrupt());
    }
    // No active-tail source has been folded yet. Recapture it in this unit's
    // snapshot if publication advanced it while we scrubbed sealed prefixes.
    // A rollover makes the former tail a sealed unit; subsequent invocations
    // continue from that unit without rescanning already certified segments.
    if progress.next_ordinal == captured.active_ordinal
        && (current != captured || agent != &progress.captured_agent)
    {
        if current != captured && current.revision == captured.revision {
            return Err(corrupt());
        }
        progress.captured_head = current.clone();
        progress.captured_agent = agent.clone();
        progress.through_seq = current.last_seq;
        progress.head_revision = current.revision;
    }
    let rebuilt = rebuild::head(progress);
    let captured = &progress.captured_head;
    if database.collection::<Document>(SEGMENTS).find_one(doc! {
        "agent_id":&current.agent_id,"incarnation":&current.incarnation,"ordinal":{"$gte":current.active_ordinal},
    }).projection(doc! {"_id":1}).session(&mut *session).await.map_err(unavailable)?.is_some() {
        return Err(corrupt());
    }
    let ordinal = progress.next_ordinal;
    let tail = ordinal == captured.active_ordinal;
    let seal = if ordinal < current.active_ordinal {
        Some(Seal::read(database.collection::<Document>(SEGMENTS).find_one(doc! {
            "agent_id":&progress.agent_id,"incarnation":&progress.incarnation,"ordinal":ordinal,
        }).session(&mut *session).await.map_err(unavailable)?.ok_or_else(corrupt)?)?)
    } else {
        None
    };
    let (start, end, expected_bytes) = if tail {
        if progress.previous_seal != captured.previous_seal {
            return Err(corrupt());
        }
        (
            captured.active_start_seq,
            captured.last_seq,
            captured.active_bytes,
        )
    } else {
        let seal = seal.as_ref().ok_or_else(corrupt)?;
        if seal.previous_seal != progress.previous_seal
            || seal.prefix_bytes
                != progress
                    .checked_bytes
                    .checked_add(seal.encoded_bytes)
                    .ok_or_else(corrupt)?
        {
            return Err(corrupt());
        }
        (seal.start_seq, seal.end_seq, seal.encoded_bytes)
    };
    if start != progress.checked_entries + 1 || end < start - 1 || end - start + 1 > MAX_ENTRIES {
        return Err(corrupt());
    }
    if let Some(seal) = &seal {
        if seal.agent_id != progress.agent_id
            || seal.incarnation != progress.incarnation
            || seal.ordinal != ordinal
            || seal.start_seq != start
            || seal.end_seq < end
            || seal.previous_seal != progress.previous_seal
        {
            return Err(corrupt());
        }
    }
    let raw = database.collection::<RawDocumentBuf>("agent_log");
    let mut rows = raw
        .find(doc! {"agent_id":&progress.agent_id,"seq":{"$gte":start,"$lte":end}})
        .sort(doc! {"seq":1})
        // Mongo additionally caps each reply by its BSON message budget. A
        // fixed row batch avoids one round trip per event in a full segment.
        .batch_size(128)
        .limit(MAX_ENTRIES + 1)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut next = start;
    let mut bytes = 0_i64;
    loop {
        let next_row = rows
            .stream(&mut *session)
            .try_next()
            .await
            .map_err(unavailable)?;
        let Some(raw) = next_row else {
            break;
        };
        let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
        if entry.agent_id.as_str() != progress.agent_id
            || i64::try_from(entry.seq).ok() != Some(next)
            || next > end
        {
            return Err(corrupt());
        }
        if (next == 1)
            != matches!(
                entry.kind,
                layer2_core::sandbox::AgentLogKind::Created { .. }
            )
        {
            return Err(corrupt());
        }
        bytes = bytes
            .checked_add(i64::try_from(raw.as_bytes().len()).map_err(|_| corrupt())?)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or_else(corrupt)?;
        if let Some(seal) = &seal {
            digest::check_leaf(
                &seal.leaf_digests,
                next - start,
                &digest::leaf(
                    &progress.agent_id,
                    &progress.incarnation,
                    next,
                    raw.as_bytes(),
                ),
            )?;
        }
        let queued = layer2_core::sandbox::AgentLogProjectionState::queued_owner(&entry);
        if let Some(sequence) = queued {
            let key = sequence.to_string();
            let kind = "queued";
            let source = database
                .collection::<Document>(super::EVIDENCE)
                .find_one(doc! {
                    "_id":super::projection::identity(&rebuilt, kind, &key),
                })
                .session(&mut *session)
                .await
                .map_err(unavailable)?
                .ok_or_else(corrupt)?;
            let evidence: super::projection::Evidence =
                bson::deserialize_from_document(source).map_err(|_| corrupt())?;
            if !matches!(evidence.record, super::projection::EvidenceRecord::Queued { sequence: original, .. } if original == sequence && sequence < entry.seq)
            {
                return Err(corrupt());
            }
        }
        let changes = progress
            .semantic
            .observe(&progress.captured_agent, &entry, queued)
            .map_err(SandboxStoreError::Corrupt)?;
        for change in changes {
            let document = super::projection::plan(&rebuilt, change)?;
            let evidence: super::projection::Evidence =
                bson::deserialize_from_document(document).map_err(|_| corrupt())?;
            database
                .collection::<Document>(super::EVIDENCE)
                .replace_one(
                    doc! {"_id":&evidence.id},
                    bson::serialize_to_document(&evidence).map_err(|_| corrupt())?,
                )
                .upsert(true)
                .session(&mut *session)
                .await
                .map_err(unavailable)?;
        }
        next += 1;
    }
    if next != end + 1 || bytes != expected_bytes {
        return Err(corrupt());
    }
    progress.checked_entries = end;
    progress.checked_bytes = progress
        .checked_bytes
        .checked_add(bytes)
        .ok_or_else(corrupt)?;
    progress.previous_seal = seal.map(|s| s.seal_digest);
    progress.next_ordinal += 1;
    if tail {
        if progress.checked_entries != captured.last_seq
            || progress.checked_bytes != captured.lifetime_bytes
        {
            return Err(corrupt());
        }
        progress
            .semantic
            .validate_aggregate(&progress.captured_agent)
            .map_err(SandboxStoreError::Corrupt)?;
        if raw
            .find_one(doc! {"agent_id":&progress.agent_id,"seq":{"$gt":current.last_seq}})
            .projection(doc! {"_id":1})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .is_some()
        {
            return Err(corrupt());
        }
        if progress.semantic.through_seq != captured.last_seq as u64
            || progress.semantic.steering.is_some()
        {
            return Err(corrupt());
        }
        rebuild::validate_queue(database, progress, session).await?;
        progress.status = "complete".into();
    }
    Ok(())
}

#[cfg(test)]
pub(in crate::store) async fn claim_for_test(
    client: &Client,
    database: &Database,
    agent: &str,
    audit: &str,
) -> Result<Document, SandboxStoreError> {
    let mut session = client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let claimed = progress::claim(database, agent, audit, false, &mut session).await?;
    commit_unit(&mut session).await?;
    bson::serialize_to_document(&claimed).map_err(|_| corrupt())
}

#[cfg(test)]
pub(in crate::store) async fn finish_for_test(
    client: &Client,
    database: &Database,
    claim: Document,
) -> Result<String, SandboxStoreError> {
    let mut session = client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let claimed = bson::deserialize_from_document(claim).map_err(|_| corrupt())?;
    let result = finish(database, claimed, &mut session).await;
    if result.is_ok() {
        commit_unit(&mut session).await?;
    } else {
        let _ = session.abort_transaction().await;
    }
    result
}
