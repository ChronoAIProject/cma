//! Atomic successor allocation for an explicitly authorized failed Agent recovery.
use super::agent_worktree_recovery_records::{self as records, ApplyError};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, wait_for_reconciliation,
    CommitOutcome, MongoSandboxStore,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::{
    plan_agent_worktree_recovery, store::AgentControlStoreError as Domain, Agent,
    AgentWorktreeRecoveryContext, AgentWorktreeRecoveryMutation,
};
use layer2_core::{AgentId, Caller, WorktreeId};

pub(super) async fn begin(
    store: &MongoSandboxStore,
    id: &AgentId,
    caller: &Caller,
    operation_id: &str,
    successor_id: &WorktreeId,
    now: String,
) -> Result<Agent, Domain> {
    let mut last_error = None;
    for attempt in 0..16 {
        super::workspace_admission::retry::retry_delay(attempt).await;
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_error = Some(error.to_string());
                continue;
            }
            return Err(unavailable(error));
        }
        let result = apply(
            store,
            id,
            caller,
            operation_id,
            successor_id,
            &now,
            &mut session,
        )
        .await;
        let (agent, mutation) = match result {
            Ok(value) => value,
            Err(error) => {
                let _ = session.abort_guarded().await;
                match error {
                    ApplyError::Driver(error) if is_transient(&error) => {
                        last_error = Some(error.to_string());
                        continue;
                    }
                    ApplyError::Driver(error) if is_duplicate_key(&error) => {
                        return Err(Domain::WorktreeRecoveryConflict)
                    }
                    ApplyError::Driver(error) => return Err(unavailable(error)),
                    ApplyError::Domain(error) => return Err(error),
                }
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(agent),
            CommitOutcome::Retry(error) => last_error = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                // Never allocate again after an uncertain commit. Prove the exact
                // archived request and both worktrees at one durable snapshot.
                let Some(expected) = mutation else {
                    return Err(unavailable(error));
                };
                let timestamp = session.operation_time().ok_or_else(|| {
                    Domain::Unavailable("worktree recovery commit has no operation time".into())
                })?;
                let store = store.clone();
                return wait_for_reconciliation("agent worktree recovery", async move {
                    records::reconcile(&store, &expected, timestamp).await
                })
                .await
                .map_err(Domain::Unavailable)?;
            }
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
        }
    }
    Err(Domain::Unavailable(format!(
        "worktree recovery exhausted its transaction budget: {}",
        last_error.unwrap_or_else(|| "no driver result".into())
    )))
}

async fn apply(
    store: &MongoSandboxStore,
    id: &AgentId,
    caller: &Caller,
    operation_id: &str,
    successor_id: &WorktreeId,
    now: &str,
    session: &mut mongodb::ClientSession,
) -> Result<(Agent, Option<AgentWorktreeRecoveryMutation>), ApplyError> {
    let current = records::load(store, id, caller, session).await?;
    let plan = |live_agents| {
        plan_agent_worktree_recovery(
            AgentWorktreeRecoveryContext {
                agent: &current.agent,
                worktree: &current.worktree,
                workspace: &current.workspace,
                sandbox: &current.sandbox,
                caller,
                live_agents,
            },
            operation_id,
            successor_id,
            now.into(),
        )
    };
    // Replay precedes quota and cleanup fences. Never resurrect a later deleted
    // Agent or consume another worktree number for the same durable operation.
    if plan(0)?.is_none() {
        return Ok((current.agent, None));
    }
    records::fence_parents(store, &current, session).await?;
    let live = super::shared_child_capacity::agents(
        store,
        &current.agent.sandbox_id,
        &current.agent.workspace_id,
        session,
    )
    .await?;
    let mutation =
        plan(u8::try_from(live).unwrap_or(u8::MAX))?.ok_or(Domain::WorktreeRecoveryConflict)?;
    records::replace(store, &current, &mutation, session).await?;
    Ok((mutation.agent.clone(), Some(mutation)))
}

fn unavailable(error: mongodb::error::Error) -> Domain {
    Domain::Unavailable(error.to_string())
}
