use super::super::{ordinary, visit_range, LogRange};
use super::*;
use layer2_core::sandbox::{store::SandboxStoreError, AgentLogEntry};
use std::collections::{BTreeMap, BTreeSet};

/// Internal to one settlement call: no captured handle escapes the borrowed
/// transaction. Authenticate selected closed points and visit the active tail
/// once, retaining only the fixed set of original command facts.
pub(super) async fn read(
    store: &MongoSandboxStore,
    head: &Head,
    requested: [Option<u64>; 7],
    session: &mut ClientSession,
) -> Result<BTreeMap<u64, AgentLogEntry>> {
    let requested: BTreeSet<_> = requested.into_iter().flatten().collect();
    if requested
        .iter()
        .any(|seq| *seq == 0 || *seq > head.last_seq as u64)
    {
        return Err(corrupt().into());
    }
    let mut rows = BTreeMap::new();
    let mut retained_bytes = 0;
    let mut retain = |entry: AgentLogEntry| {
        if requested.contains(&entry.seq) {
            retained_bytes += serde_json::to_vec(&entry).map_err(|_| corrupt())?.len();
            if retained_bytes > 4 * 1024 * 1024 {
                return Err(SandboxStoreError::Unavailable(
                    "command source points exceed the bounded evidence budget".into(),
                ));
            }
            if rows.insert(entry.seq, entry).is_some() {
                return Err(corrupt());
            }
        }
        Ok(())
    };
    for sequence in requested
        .iter()
        .copied()
        .filter(|seq| *seq < head.active_start_seq as u64)
    {
        retain(ordinary::point(store, head, sequence, session).await?)?;
    }
    visit_range(
        store,
        head,
        LogRange {
            after: head.active_start_seq as u64 - 1,
            through: head.last_seq as u64,
            limit: u16::try_from(head.active_count).map_err(|_| corrupt())?,
        },
        session,
        false,
        &mut retain,
    )
    .await?;
    if rows.len() != requested.len() {
        return Err(corrupt().into());
    }
    Ok(rows)
}
