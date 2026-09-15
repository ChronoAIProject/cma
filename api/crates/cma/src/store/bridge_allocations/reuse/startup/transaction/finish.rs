use super::*;
use crate::sandbox::channel_provisioning::state::ChannelProvisioningPhase;
use layer2_core::{
    bridge::allocation::{AllocationResourceReceipt, AllocationState},
    sandbox::{
        AgentLogEntry, AgentLogKind, AgentStatus, AgentWorkReceipt, AgentWorkStep,
        SandboxEventKind, WorktreeStatus,
    },
};

// Each supplied receipt/resource participates in the same publication transaction.
#[allow(clippy::too_many_arguments)]
pub(super) async fn apply(
    store: &MongoBridgeAllocationStore,
    database: &Database,
    mut context: ChannelReuseStartupContext,
    plan: &ChannelStartupPlan,
    receipt: &WorktreeReceipt,
    conversation: &str,
    resources: &MongoSandboxStore,
    session: &mut ClientSession,
) -> Result<ChannelReuseStartupContext, MutationError> {
    context.require_plan(plan)?;
    let claim = context
        .snapshot
        .allocation
        .lease
        .as_ref()
        .ok_or(AllocationError::LeaseUnavailable)?;
    if context.plan.as_ref() != Some(plan)
        || !context.valid_worktree(plan, receipt)
        || context.agent.work.next_fence != claim.fence
        || conversation.trim() != conversation
        || conversation.is_empty()
        || conversation.len() > 512
        || conversation.chars().any(char::is_control)
        || resources.sandboxes.namespace().db != database.name()
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let runtime = context
        .snapshot
        .sandbox
        .runtime
        .as_ref()
        .ok_or(AllocationError::Corrupt)?;
    let runtime_generation = runtime
        .provision_generation
        .ok_or(AllocationError::Corrupt)?;
    crate::store::channel_tools::validate_and_fence_open(
        resources,
        &layer3_adapter::RuntimeHandle::new(&runtime.runtime_id),
        &plan.open,
        claim.fence,
        conversation,
        session,
    )
    .await
    .map_err(|_| AllocationError::AuthorityUnavailable)?;
    let stamp = layer2_core::rfc3339_at(std::time::SystemTime::now());
    let agent_revision = context.agent.revision;
    let tree_revision = context.worktree.revision;
    let previous_allocation = context.snapshot.allocation.clone();
    context.agent.conversation.conversation_id = Some(conversation.into());
    context.agent.conversation.runtime_generation = Some(runtime_generation);
    context.agent.work.receipts = plan
        .intents()
        .into_iter()
        .map(|intent| AgentWorkReceipt {
            step: intent.step,
            operation_id: intent.operation_id,
            request_sha256: intent.request_sha256,
            completed_at: stamp.clone(),
        })
        .collect();
    // Each uncertain intent is now covered by its successful receipt. The
    // ordinary lifecycle ledger forbids the same effect in both histories.
    context.agent.work.cleanup_debt.clear();
    context.agent.work.next_fence = claim.fence.checked_add(1).ok_or(AllocationError::Corrupt)?;
    context
        .agent
        .work
        .schedule(AgentWorkStep::PublishConnection, stamp.clone());
    context
        .agent
        .transition(AgentStatus::Ready, stamp.clone(), None)
        .map_err(|_| AllocationError::Corrupt)?;
    context
        .worktree
        .transition(WorktreeStatus::Ready)
        .map_err(|_| AllocationError::Corrupt)?;
    let logs = [
        AgentLogKind::ConversationOpened {
            conversation_id: conversation.into(),
            runtime_generation,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    let rows: Collection<Document> = majority_collection(database, "agent_log");
    for kind in logs {
        context.agent.conversation.last_seq = context
            .agent
            .conversation
            .last_seq
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        let log = AgentLogEntry {
            agent_id: context.agent.id.clone(),
            seq: context.agent.conversation.last_seq,
            appended_at: stamp.clone(),
            kind,
        };
        rows.insert_one(
            crate::store::agent_log::log_document(&log).map_err(|_| AllocationError::Corrupt)?,
        )
        .session(&mut *session)
        .await?;
    }
    let previous_snapshot = context.snapshot.clone();
    context.snapshot.state = context.snapshot.state.advance(
        ChannelProvisioningPhase::Ready,
        &(
            plan,
            receipt,
            conversation,
            runtime_generation,
            &context.creation,
        ),
    )?;
    // The startup plan is durable before any effect; retain it with the final receipt.
    context.snapshot.state.startup_plan = Some(plan.clone());
    let allocation = &mut context.snapshot.allocation;
    allocation.state = AllocationState::Ready;
    allocation.receipt = Some(AllocationResourceReceipt {
        operation_id: allocation.operation_id.clone(),
        target: allocation.placement.target().clone(),
        profiles_digest: allocation.profiles_digest().map(str::to_owned),
        receipt_digest: context
            .snapshot
            .state
            .last_receipt_digest
            .clone()
            .ok_or(AllocationError::Corrupt)?,
    });
    allocation.lease = None;
    allocation.revision = allocation
        .revision
        .checked_add(1)
        .ok_or(AllocationError::Corrupt)?;
    allocation.updated_at_ms = mongodb::bson::DateTime::now().timestamp_millis();
    allocation.validate()?;
    context.snapshot.agent = Some(context.agent.clone());
    context.snapshot.worktree = Some(context.worktree.clone());
    save_agent(database, &context, agent_revision, session).await?;
    let mut tree = crate::store::worktrees::worktree_document(&context.worktree)
        .map_err(|_| AllocationError::Corrupt)?;
    tree.remove("_id");
    replace_fields(
        database,
        "worktrees",
        context.worktree.id.as_str(),
        tree_revision,
        tree,
        session,
    )
    .await?;
    // Only child status events are appended. The shared root lifecycle is unchanged.
    reuse_transaction::event(
        resources,
        &context.snapshot,
        SandboxEventKind::AgentStatusChanged {
            agent_id: context.agent.id.clone(),
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
        session,
    )
    .await?;
    reuse_transaction::save_state(store, &previous_snapshot, &context.snapshot.state, session)
        .await?;
    store
        .replace(&previous_allocation, &context.snapshot.allocation, session)
        .await?;
    Ok(context)
}

async fn replace_fields(
    database: &Database,
    collection: &str,
    id: &str,
    revision: u64,
    fields: Document,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let rows: Collection<Document> = majority_collection(database, collection);
    let changed = rows
        .update_one(
            doc! {"_id":id,"revision":revision as i64},
            doc! {"$set":fields},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(())
}
