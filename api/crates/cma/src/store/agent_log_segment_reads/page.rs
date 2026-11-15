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
        return Ok(None);
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
