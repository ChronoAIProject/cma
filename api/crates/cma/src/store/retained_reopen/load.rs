use futures_util::TryStreamExt;
use layer2_core::{
    sandbox::{RetainedRecoverySnapshot, RetainedReopenSnapshot},
    SandboxId,
};
use mongodb::{bson::doc, ClientSession};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

use super::{domain, ApplyError, MongoSandboxStore};
use crate::store::{
    agent_commands::{lane_records, records},
    agent_log_integrity::IntegrityFailurePolicy,
    sandbox_events::validate_in_transaction,
    sandbox_unpublished_recovery::transaction::{
        load_reopen_with_sources, load_suspended_reopen_with_sources, NativeFacts,
    },
    worktrees::read_worktree,
};

pub(super) async fn snapshot_with_sources(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
    sources: Option<&NativeFacts>,
    policy: IntegrityFailurePolicy,
) -> Result<(RetainedReopenSnapshot, u64), ApplyError> {
    let (original, last_seq, history_state_sha256) =
        load_reopen_with_sources(store, id, session, sources, policy).await?;
    complete_snapshot(store, id, session, original, last_seq, history_state_sha256).await
}

pub(super) async fn suspended_snapshot(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
    sources: &NativeFacts,
    integrity_policy: IntegrityFailurePolicy,
) -> Result<(RetainedReopenSnapshot, u64), ApplyError> {
    let (original, last_seq, history_state_sha256) =
        load_suspended_reopen_with_sources(store, id, session, sources, integrity_policy).await?;
    let policy = store
        .resource_policies
        .find_one(doc! {"_id":id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    super::suspended::require_personal(policy.as_ref(), &original.sandbox)
        .map_err(ApplyError::Domain)?;
    complete_snapshot(store, id, session, original, last_seq, history_state_sha256).await
}

async fn complete_snapshot(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
    original: layer2_core::sandbox::SandboxUnpublishedRecoverySnapshot,
    last_seq: u64,
    history_state_sha256: String,
) -> Result<(RetainedReopenSnapshot, u64), ApplyError> {
    validate_in_transaction(store, id, last_seq, session).await?;
    let mut cursor = store
        .worktrees
        .find(doc! { "sandbox_id": id.as_str() })
        .sort(doc! { "_id": 1 })
        .limit(17)
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let mut worktrees = Vec::new();
    while let Some(row) = cursor
        .stream(&mut *session)
        .try_next()
        .await
        .map_err(ApplyError::Driver)?
    {
        worktrees.push(read_worktree(&row).map_err(ApplyError::Domain)?);
    }
    let ids = original
        .agents
        .iter()
        .map(|a| a.id.as_str())
        .collect::<Vec<_>>();
    let mut cursor = store
        .agent_commands
        .find(doc! { "agent_id": { "$in": ids } })
        .sort(doc! { "_id": 1 })
        .limit(4097)
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let mut digest = Sha256::new();
    let mut count = 0;
    let mut orders = BTreeMap::<_, BTreeSet<_>>::new();
    while let Some(row) = cursor
        .stream(&mut *session)
        .try_next()
        .await
        .map_err(ApplyError::Driver)?
    {
        count += 1;
        let command = records::read(&row).map_err(|_| domain("recovery command is invalid"))?;
        if count > 4096 || !command.status.is_terminal() || command.work.lease.is_some() {
            return Err(domain("recovery command inventory is not idle"));
        }
        if !orders
            .entry(command.receipt.envelope.agent_id.clone())
            .or_default()
            .insert(command.receipt.agent_order)
        {
            return Err(domain("recovery command order is duplicated"));
        }
        // queued_input is a versioned sidecar skipped by the ordinary record
        // serializer; it is still part of the complete retained command state.
        let bytes = serde_json::to_vec(&(&command, &command.queued_input))
            .map_err(|_| domain("recovery command cannot be encoded"))?;
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    let mut command_lanes = Vec::new();
    for agent in &original.agents {
        let lane = store
            .agent_command_orders
            .find_one(doc! { "_id": agent.id.as_str() })
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .map(|row| lane_records::read(&row, agent.id.as_str()))
            .transpose()
            .map_err(|_| domain("recovery command lane is invalid"))?
            .unwrap_or_default();
        let retained = orders.remove(&agent.id).unwrap_or_default();
        if lane.last_order != retained.len() as u64
            || retained.iter().copied().ne(1..=lane.last_order)
        {
            return Err(domain("recovery command inventory is incomplete"));
        }
        command_lanes.push((agent.id.clone(), lane));
    }
    Ok((
        RetainedReopenSnapshot {
            retained: RetainedRecoverySnapshot {
                sandbox: original.sandbox,
                agents: original.agents,
                workspaces: original.workspaces,
            },
            worktrees,
            history_state_sha256,
            command_lanes,
            event_seq: last_seq,
            command_state_sha256: format!("{:x}", digest.finalize()),
        },
        last_seq,
    ))
}
