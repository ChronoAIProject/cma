//! Bound the compatibility vector; indexed consumers use their own page ports.

use super::*;
use crate::store::agent_log_segments::{self, Head, LogRange};

/// This compatibility API returns every requested row or an explicit refusal.
/// Paging the underlying query cannot remove this vector's total memory bound.
/// Large-history response and transcript consumers must use their page ports.
pub(super) async fn read_compatibility_window(
    store: &MongoSandboxStore,
    head: &ReadHead,
    segmented: &Head,
    after: u64,
    through: u64,
    session: &mut ClientSession,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let count = through
        .checked_sub(after)
        .ok_or(SandboxStoreError::InvalidPage)?;
    if count > agent_log_segments::MAX_ENTRIES as u64 {
        return Err(SandboxStoreError::AgentLogCapacityExceeded);
    }
    let mut entries = Vec::new();
    let mut bytes = 0_usize;
    // Even an empty request must check the captured tail/seal proof. In
    // particular, a future cursor cannot hide an uncommitted source append.
    agent_log_segments::visit_range(
        store,
        segmented,
        LogRange {
            after: after.min(head.last_seq),
            through: through.min(head.last_seq),
            limit: u16::try_from(count).map_err(|_| SandboxStoreError::InvalidPage)?,
        },
        session,
        true,
        |entry| {
            head.validate_lineage(&entry)?;
            bytes = bytes.saturating_add(
                serde_json::to_vec(&entry)
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
                    .len(),
            );
            if bytes > 4 * 1024 * 1024 {
                return Err(SandboxStoreError::AgentLogCapacityExceeded);
            }
            entries.push(entry);
            Ok(())
        },
    )
    .await
    .map_err(transaction_error)?;
    Ok(entries)
}
