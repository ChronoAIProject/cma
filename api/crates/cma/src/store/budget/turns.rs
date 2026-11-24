//! Retained turn accounting shared by conversation and runtime-loss publication.
use super::*;
use crate::store::agent_log_integrity::TransactionLogError;
use layer2_core::sandbox::store::{SandboxRuntimeLossAgentMutation, SandboxStoreError};

struct ExistingTurn<'a> {
    current: &'a Agent,
    ended: bool,
    usage: Option<(u64, u64)>,
    now: u64,
}

pub(super) async fn apply_conversation(
    store: &MongoSandboxStore,
    current: &Agent,
    mutation: &AgentConversationMutation,
    now: u64,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let usage = mutation
        .agent
        .conversation
        .observations
        .usage
        .as_ref()
        .and_then(|usage| usage.current_turn.as_ref())
        .filter(|usage| Some(usage.turn_sequence) == current.conversation.active_turn)
        .filter(|_| {
            mutation
                .agent_logs
                .iter()
                .any(|log| matches!(log, AgentLogKind::UsageUpdated { .. }))
        })
        .map(|usage| (usage.input_tokens, usage.output_tokens));
    apply_existing(
        store,
        ExistingTurn {
            current,
            ended: mutation.agent.conversation.active_turn != current.conversation.active_turn,
            usage,
            now,
        },
        session,
    )
    .await
}

pub(in crate::store) async fn settle_runtime_loss(
    store: &MongoSandboxStore,
    current: &Agent,
    change: &SandboxRuntimeLossAgentMutation,
    at: &str,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    let Some(turn) = current.conversation.active_turn else {
        return Ok(());
    };
    if change.agent.conversation.active_turn == Some(turn) {
        return Ok(());
    }
    let invalid = || {
        TransactionLogError::Domain(SandboxStoreError::Corrupt(
            "runtime-loss budget closure does not match the captured turn".into(),
        ))
    };
    if current.id != change.agent.id
        || current.owner != change.agent.owner
        || current.sandbox_id != change.agent.sandbox_id
        || current.workspace_id != change.agent.workspace_id
        || current.worktree_id != change.agent.worktree_id
        || current.revision != change.expected_revision
        || current.conversation.last_seq != change.expected_last_seq
        || change.agent.conversation.active_turn.is_some()
        || !interrupts_only(&change.logs, turn)
    {
        return Err(invalid());
    }
    let now = layer2_core::seconds_from_rfc3339(at).ok_or_else(invalid)?;
    apply_existing(
        store,
        ExistingTurn {
            current,
            ended: true,
            usage: None,
            now,
        },
        session,
    )
    .await
    .map_err(|error| match error {
        ApplyError::Domain(error) => TransactionLogError::Domain(error),
        ApplyError::Driver(error) => TransactionLogError::Driver(error),
    })
}

pub(in crate::store) async fn settle_owner_stop(
    store: &MongoSandboxStore,
    current: &Agent,
    appended_at: &str,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let now = layer2_core::seconds_from_rfc3339(appended_at).ok_or_else(|| {
        ApplyError::Domain(SandboxStoreError::Corrupt(
            "invalid owner Stop timestamp".into(),
        ))
    })?;
    apply_existing(
        store,
        ExistingTurn {
            current,
            ended: true,
            usage: None,
            now,
        },
        session,
    )
    .await
}

fn interrupts_only(logs: &[AgentLogKind], turn: u64) -> bool {
    let mut interrupted = false;
    for log in logs {
        match log {
            AgentLogKind::TurnInterrupted { turn_sequence }
                if *turn_sequence == turn && !interrupted =>
            {
                interrupted = true
            }
            AgentLogKind::TurnInterrupted { .. }
            | AgentLogKind::TurnStarted { .. }
            | AgentLogKind::TurnCompleted { .. }
            | AgentLogKind::TurnFailed { .. } => return false,
            _ => {}
        }
    }
    interrupted
}

async fn apply_existing(
    store: &MongoSandboxStore,
    update: ExistingTurn<'_>,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let current = update.current;
    let now = update.now;
    let Some(turn) = current.conversation.active_turn else {
        return Ok(());
    };
    let key = turn_key(current, turn);
    let Some(document) = crate::store::stale_turns::find_with_proof_budget(
        &store.model_budget,
        doc! {"_id": &key},
        &mut *session,
        false,
    )
    .await
    .map_err(ApplyError::Driver)?
    else {
        return Ok(());
    };
    let mut record: TurnRecord = decode(&document)?;
    if record.owner != current.owner
        || record.agent_id != current.id
        || record.turn_sequence != Some(turn)
    {
        return Err(domain(BudgetError::Unavailable));
    }
    if record.reservation.settled {
        return Ok(());
    }
    if let Some((input, output)) = update.usage {
        if let Err(reason) = record.reservation.observe(input, output, now) {
            record.reservation.stop.get_or_insert(reason);
        }
    }
    if update.ended
        || record
            .reservation
            .observed
            .saturating_add(record.reservation.policy.max_unobserved_units)
            > record.reservation.reserved_units
    {
        let mut owner: OwnerBudget = crate::store::stale_turns::find_with_proof_budget(
            &store.model_budget,
            doc! {"_id": owner_key(current)},
            &mut *session,
            false,
        )
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(decode)
        .transpose()?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
        // Existing observations do not prove a complete provider settlement.
        // Keep the full allowance as a debit until an authoritative source can reconcile.
        let before = record.reservation.reserved_units;
        owner
            .retain_observed_liability(&mut record.reservation)
            .map_err(domain)?;
        let extra = record.reservation.reserved_units - before;
        let ended = update.ended;
        if ended {
            owner.settle(&record.reservation, now).map_err(domain)?;
            record.reservation.settled = true;
        }
        if extra > 0 || ended {
            let (reserved, debit) = (
                record.reservation.reserved_units,
                record.reservation.settlement_debit(),
            );
            counter_accounting::mirror(
                store,
                record.period_start,
                record.period_shard,
                counters::Change {
                    retained: extra,
                    released: if ended { reserved } else { 0 },
                    debit: if ended { debit } else { 0 },
                },
                session,
            )
            .await?;
        }
        save(store, &owner_key(current), &owner, session).await?;
    }
    save(store, &key, &record, session).await
}
