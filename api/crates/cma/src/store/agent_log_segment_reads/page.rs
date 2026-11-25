//! Certified compatibility event pages, bounded independently of lifetime size.
use super::*;
use segments::capture;

const MAX_BYTES: usize = 4 * 1024 * 1024;

pub(in crate::store) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    after: u64,
    limit: u16,
) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError> {
    if limit == 0 || u64::from(limit) > PAGE_SIZE {
        return Err(SandboxStoreError::InvalidPage);
    }
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    if !is_segmented(store, agent, &mut session).await? {
        return legacy(store, agent, after, limit, &mut session).await;
    }
    if let Some(page) = sealed_page(store, agent, after, limit, &mut session).await? {
        return Ok(Some(page));
    }
    let source = capture::capture(store, agent, &mut session, capture::CaptureMode::Snapshot)
        .await
        .map_err(segment_error)?;
    let mut entries = Vec::new();
    let mut bytes = 2;
    let end = source.through().min(after.saturating_add(u64::from(limit)));
    let mut cursor = after;
    while cursor < end {
        let page = points(store, &source, &mut session, cursor, end.min(cursor + 32)).await?;
        for entry in page {
            let size = serde_json::to_vec(&entry)
                .map_err(|_| SandboxStoreError::Corrupt("invalid Agent event".into()))?
                .len()
                + usize::from(!entries.is_empty());
            if bytes + size > MAX_BYTES {
                if entries.is_empty() {
                    return Err(SandboxStoreError::AgentLogCapacityExceeded);
                }
                return Ok(Some(entries));
            }
            bytes += size;
            cursor = entry.seq;
            entries.push(entry);
        }
    }
    Ok(Some(entries))
}

/// A sealed page can verify its original rows against immutable leaf digests
/// without materializing unrelated active history. Keep semantic projection
/// and head validation in the same source snapshot.
async fn sealed_page(
    store: &MongoSandboxStore,
    agent: &Agent,
    after: u64,
    limit: u16,
    session: &mut ClientSession,
) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError> {
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .ok_or(SandboxStoreError::NotFound)?;
    if crate::store::agents::read_agent(&row)? != *agent {
        return Err(SandboxStoreError::SequenceConflict);
    }
    let head = segments::read_head(store, &row, session, true)
        .await
        .map_err(segment_error)?;
    let through = agent
        .conversation
        .last_seq
        .min(after.saturating_add(u64::from(limit)));
    if after >= through || through >= head.active_start_seq as u64 {
        return Ok(None);
    }
    segments::verify_terminal_seal(store, &head, session)
        .await
        .map_err(segment_error)?;
    segments::projection::read(store, agent, &head, session)
        .await
        .map_err(segment_error)?;
    let mut entries = Vec::new();
    let mut bytes = 2;
    let mut full = false;
    segments::visit_range(
        store,
        &head,
        LogRange {
            after,
            through,
            limit,
        },
        session,
        true,
        |entry| {
            validate_lineage(agent, &entry)?;
            if full {
                return Ok(());
            }
            let size = serde_json::to_vec(&entry)
                .map_err(|_| SandboxStoreError::Corrupt("invalid Agent event".into()))?
                .len()
                + usize::from(!entries.is_empty());
            if bytes + size > MAX_BYTES {
                if entries.is_empty() {
                    return Err(SandboxStoreError::AgentLogCapacityExceeded);
                }
                full = true;
            } else {
                bytes += size;
                entries.push(entry);
            }
            Ok(())
        },
    )
    .await
    .map_err(segment_error)?;
    Ok(Some(entries))
}

/// Prepared legacy histories already certify the semantic prefix. Reuse that
/// proof in the source snapshot; unprepared histories retain complete validation.
async fn legacy(
    store: &MongoSandboxStore,
    agent: &Agent,
    after: u64,
    limit: u16,
    session: &mut ClientSession,
) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError> {
    use crate::store::agent_log_reads::{integrity::CapturedIntegrity, read_bounded};
    let head = CapturedIntegrity(agent)
        .read_head(store, &agent.id, session)
        .await?
        .ok_or(SandboxStoreError::NotFound)?;
    if !head.trusted {
        return Ok(None);
    }
    let after = after.min(agent.conversation.last_seq);
    let through = agent.conversation.last_seq.min(after + u64::from(limit));
    // Legacy pages retain their existing payload contract. The physical source
    // is bounded by the unchanged legacy BSON budget; no segment writer changes.
    read_bounded(store, &agent.id, after, through, &head, session, usize::MAX).await
}

/// A requested row count is an upper bound. Halving a 32-row page needs at
/// most five retries, retains at most 4 MiB, and keeps every retry in the same
/// certified snapshot. The caller advances only past admitted source rows.
pub(super) async fn points(
    store: &MongoSandboxStore,
    source: &capture::CapturedLog,
    session: &mut ClientSession,
    after: u64,
    mut through: u64,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    loop {
        match capture::points(
            store,
            source,
            &(after + 1..=through).collect::<Vec<_>>(),
            session,
            capture::CaptureMode::Snapshot,
        )
        .await
        .map_err(segment_error)
        {
            Err(SandboxStoreError::AgentLogCapacityExceeded) if through > after + 1 => {
                through = after + (through - after) / 2;
            }
            result => return result,
        }
    }
}
