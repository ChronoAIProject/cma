//! Bounded original-history identity for a consented retained reopen.
use super::*;
use crate::store::agent_log_segments::{capture, transaction_scope::TransactionScope};

pub(super) async fn proof(
    store: &MongoSandboxStore,
    source: &capture::CapturedLog,
    scope: &mut TransactionScope<'_>,
) -> Result<Vec<u8>, ApplyError> {
    let head = source.active_head();
    // Reserve the same normal-capacity startup allowance as legacy reopening.
    // Reopening itself does not admit rollover or consume stop/delete reserves.
    crate::store::agent_log_integrity::ensure_reopen_capacity_for(
        head.active_count as u64,
        head.active_bytes as usize,
    )
    .map_err(ApplyError::Domain)?;
    let creation = capture::points_transaction(store, source, &[1], scope)
        .await?
        .pop()
        .ok_or(ApplyError::Domain(SandboxStoreError::SequenceConflict))?;
    let agent = source.agent();
    if !matches!(creation.kind, AgentLogKind::Created { workspace_id, worktree_id }
        if workspace_id == agent.workspace_id && &worktree_id == agent.creation_worktree_id())
    {
        return Err(ApplyError::Domain(SandboxStoreError::Corrupt(
            "retained creation lineage differs".into(),
        )));
    }
    // The immutable previous seal binds the full closed prefix; exact active
    // leaves cover the suffix. No original text is copied into consent metadata.
    let encoded = serde_json::to_vec(&(head, source.active_leaf_digests(), source.projection()))
        .map_err(|_| {
            ApplyError::Domain(SandboxStoreError::Corrupt(
                "retained source cannot be encoded".into(),
            ))
        })?;
    let mut digest = Sha256::new();
    digest.update(b"cma.retained-reopen.segmented-source.v1\0");
    digest.update(encoded);
    Ok(digest.finalize().to_vec())
}
