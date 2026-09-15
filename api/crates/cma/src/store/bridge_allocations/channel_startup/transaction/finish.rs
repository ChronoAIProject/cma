use super::*;
use crate::sandbox::channel_provisioning::state::ChannelProvisioningPhase;
use layer2_core::{
    bridge::allocation::{AllocationResourceReceipt, AllocationState},
    sandbox::{
        AgentLogEntry, AgentLogKind, AgentStatus, AgentWorkReceipt, AgentWorkStep, SandboxEvent,
        SandboxEventKind, SandboxStatus, WorktreeStatus,
    },
};

// Each supplied receipt/resource participates in the same publication transaction.
#[allow(clippy::too_many_arguments)]
pub(super) async fn apply(
    store: &MongoBridgeAllocationStore,
    database: &Database,
    mut context: ChannelStartupContext,
    plan: &ChannelStartupPlan,
    receipt: &WorktreeReceipt,
    conversation: &str,
    resources: &MongoSandboxStore,
    session: &mut ClientSession,
) -> Result<ChannelStartupContext, MutationError> {
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
    let sandbox_revision = context.snapshot.sandbox.revision;
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
    context
        .snapshot
        .sandbox
        .transition(SandboxStatus::Active, stamp.clone(), None)
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
    context.snapshot.state = context.snapshot.state.next(
        ChannelProvisioningPhase::Ready,
        &(
            plan,
            receipt,
            conversation,
            runtime_generation,
            &context.creation,
        ),
    )?;
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
    context.snapshot.validate()?;
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
    save_sandbox(database, &context, sandbox_revision, &stamp, session).await?;
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

async fn save_sandbox(
    database: &Database,
    context: &ChannelStartupContext,
    previous_revision: u64,
    stamp: &str,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let sandbox = &context.snapshot.sandbox;
    let document = read(database, "sandboxes", sandbox.id.as_str(), session).await?;
    let mut seq = document
        .get_i64("last_seq")
        .map_err(|_| AllocationError::Corrupt)?;
    let events = [
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Installing,
            to: SandboxStatus::Active,
        },
        SandboxEventKind::AgentStatusChanged {
            agent_id: context.agent.id.clone(),
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    let rows: Collection<Document> = majority_collection(database, "sandbox_events");
    for kind in events {
        seq = seq.checked_add(1).ok_or(AllocationError::Corrupt)?;
        let event = SandboxEvent {
            sandbox_id: sandbox.id.clone(),
            seq: seq as u64,
            appended_at: stamp.into(),
            kind,
        };
        rows.insert_one(
            crate::store::sandbox_events::event_document(&event)
                .map_err(|_| AllocationError::Corrupt)?,
        )
        .session(&mut *session)
        .await?;
    }
    let mut fields = crate::store::sandboxes::sandbox_replacement_fields(sandbox)
        .map_err(|_| AllocationError::Corrupt)?;
    fields.insert(
        "channel_provisioning",
        record_bson(&context.snapshot.state).map_err(|_| AllocationError::Corrupt)?,
    );
    fields.insert("last_seq", seq);
    replace_fields(
        database,
        "sandboxes",
        sandbox.id.as_str(),
        previous_revision,
        fields,
        session,
    )
    .await
}
