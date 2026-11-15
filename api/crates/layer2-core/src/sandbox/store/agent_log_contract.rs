//! Shared adapter contract; compiled only for cross-crate verification.
use super::{AgentId, AgentLogEntry, AgentLogStore, SandboxStoreError};

pub async fn tail_page(
    store: &dyn AgentLogStore,
    id: &AgentId,
    after: u64,
    through: u64,
    limit: u16,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let mut rows = Vec::new();
    store
        .visit_agent_log_page_with_tail(id, after, through, limit, &mut |entry| {
            rows.push(entry);
            Ok(())
        })
        .await?;
    Ok(rows)
}

/// Supply a valid committed history of at least five rows. Frozen ranges are
/// intentionally older than that head; every adapter must preserve them.
pub async fn assert_tail_page_contract(store: &dyn AgentLogStore, entries: &[AgentLogEntry]) {
    assert!(entries.len() >= 5);
    let id = &entries[0].agent_id;
    let head = entries.len() as u64;
    for (after, through, limit) in [
        (0, 0, 1),
        (0, 3, 2),
        (2, 3, 2),
        (3, 3, 2),
        (head - 2, head, 2),
    ] {
        assert_eq!(
            tail_page(store, id, after, through, limit).await.unwrap(),
            entries
                [after.saturating_sub(1) as usize..through.min(after + u64::from(limit)) as usize]
        );
    }
    for (after, through, limit) in [(0, 5, 0), (0, 5, 201), (4, 3, 1)] {
        assert_eq!(
            tail_page(store, id, after, through, limit).await,
            Err(SandboxStoreError::InvalidPage)
        );
    }
    assert_eq!(
        tail_page(store, id, 0, head + 1, 1).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store
            .visit_agent_log_page_with_tail(id, 0, head, 1, &mut |_| Err(
                SandboxStoreError::InvalidPage
            ))
            .await,
        Err(SandboxStoreError::InvalidPage)
    );
}
