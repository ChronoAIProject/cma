use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::resource_access::{ResourceOwner, ResourcePolicyFence};
use layer2_core::sandbox::agent_io::TriggerExecutionProof;
use layer2_core::sandbox::store::AgentConversationMutation;

pub(in crate::store) async fn proof(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
) -> Result<TriggerExecutionProof, SandboxStoreError> {
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    super::super::sandbox_mongo::start_transaction(&mut session)
        .await
        .map_err(unavailable)?;
    let proof = proof_in_session(store, command, &mut session).await;
    let _ = session.abort_guarded().await;
    proof
}

pub(in crate::store) async fn proof_in_session(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<TriggerExecutionProof, SandboxStoreError> {
    read_proof(store, command, session, ClaimRequirement::Execution).await
}

pub(in crate::store) async fn ready_for_claim(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
) -> Result<(), SandboxStoreError> {
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    super::super::sandbox_mongo::start_transaction(&mut session)
        .await
        .map_err(unavailable)?;
    let result = read_proof(store, command, &mut session, ClaimRequirement::Readiness)
        .await
        .map(|_| ());
    let _ = session.abort_guarded().await;
    result
}

enum ClaimRequirement {
    Execution,
    Readiness,
}

async fn read_proof(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
    session: &mut ClientSession,
    requirement: ClaimRequirement,
) -> Result<TriggerExecutionProof, SandboxStoreError> {
    let row = store
        .agents
        .find_one(doc! {"_id":command.receipt.envelope.agent_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    if row.contains_key("channel_reuse_cleanup_admission") {
        return Err(SandboxStoreError::Conflict);
    }
    let agent = super::super::agents::read_agent(&row)?;
    if !agent.status.is_live() || agent.requested_stop_reason.is_some() {
        return Err(SandboxStoreError::NotFound);
    }
    let binding = require_receipt(store, &agent, session)
        .await?
        .ok_or(SandboxStoreError::Conflict)?;
    binding
        .require_command(command)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let command_row = store
        .agent_commands
        .find_one(doc! {"_id":command.receipt.envelope.command_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::Conflict)?;
    let current = super::super::agent_commands::read_command(&command_row)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    binding
        .require_command(&current)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    match requirement {
        ClaimRequirement::Execution => {
            if current.work.lease.as_ref().map(|lease| &lease.claim)
                != command.work.lease.as_ref().map(|lease| &lease.claim)
                || current.status.is_terminal()
                || current
                    .work
                    .lease
                    .as_ref()
                    .is_some_and(|lease| !lease.live_at(&layer2_core::now_rfc3339()))
            {
                return Err(SandboxStoreError::Conflict);
            }
        }
        ClaimRequirement::Readiness => {
            require_ready(store, command, &current, session).await?;
        }
    }
    let database = store.client.database(&store.agents.namespace().db);
    let launch_row =
        super::super::durability::majority_collection::<Document>(&database, "trigger_launches")
            .find_one(doc! {"_id":binding.launch_id().as_str()})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .ok_or(SandboxStoreError::Conflict)?;
    let launch = super::super::trigger_definitions::decode_launch_aggregate(&launch_row)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let receipt = super::super::durability::majority_collection::<Document>(
        &database,
        "trigger_launch_input_protocols",
    )
    .find_one(doc! {"_id":binding.launch_id().as_str()})
    .session(&mut *session)
    .await
    .map_err(unavailable)?;
    super::super::trigger_definitions::require_input_protocol_document(&launch, receipt)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let parent = store
        .sandboxes
        .find_one(doc! {"_id":agent.sandbox_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(super::super::sandboxes::read_sandbox)
        .transpose()?
        .ok_or(SandboxStoreError::NotFound)?;
    if matches!(
        parent.status,
        layer2_core::sandbox::SandboxStatus::Failed
            | layer2_core::sandbox::SandboxStatus::Retiring
            | layer2_core::sandbox::SandboxStatus::Retired
    ) {
        return Err(SandboxStoreError::NotFound);
    }
    // Existing Stage C execution guards retain their normal transaction role;
    // this proof additionally rejects any parent with an active cleanup exclusion.
    let parent_row = store
        .sandboxes
        .find_one(doc! {"_id":parent.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    if parent_row.contains_key("channel_reuse_cleanup_execution") {
        return Err(SandboxStoreError::Conflict);
    }
    let open = store.workspaces.update_one(
        doc! {"_id":agent.workspace_id.as_str(),"channel_reuse_cleanup_execution":{"$exists":false},
            "owned_workspace_cleanup_admission_v1":{"$exists":false}},
        doc! {"$inc":{"owned_workspace_execution_fence":1_i64}},
    ).session(&mut *session).await.map_err(unavailable)?;
    if open.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    let workspace = store
        .workspaces
        .find_one(doc! {"_id":agent.workspace_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(super::super::workspaces::read_workspace)
        .transpose()?
        .ok_or(SandboxStoreError::NotFound)?;
    let worktree = store
        .worktrees
        .find_one(doc! {"_id":agent.worktree_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(super::super::worktrees::read_worktree)
        .transpose()?
        .ok_or(SandboxStoreError::NotFound)?;
    if workspace.sandbox_id != parent.id
        || workspace.owner != agent.owner
        || workspace.status != layer2_core::sandbox::WorkspaceStatus::Ready
        || worktree.agent_id.as_ref() != Some(&agent.id)
        || worktree.workspace_id != workspace.id
        || worktree.sandbox_id != parent.id
        || worktree.owner != agent.owner
        || worktree.status != layer2_core::sandbox::WorktreeStatus::Ready
    {
        return Err(SandboxStoreError::Conflict);
    }
    let policy = store
        .resource_policies
        .find_one(doc! {"_id":parent.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(super::super::resource_access::read_inventory_policy)
        .transpose()
        .map_err(unavailable)?;
    let owner = policy.as_ref().map_or_else(
        || ResourceOwner::Person {
            subject: parent.owner.clone(),
        },
        |policy| policy.owner.clone(),
    );
    if owner != command.receipt.envelope.owner {
        return Err(SandboxStoreError::NotFound);
    }
    let fence = ResourcePolicyFence {
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        policy_revision: policy.as_ref().map_or(0, |policy| policy.revision),
    };
    TriggerExecutionProof::checked(binding, &launch, &current, agent, parent, owner, fence)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
}

pub(in crate::store) async fn require_mutation(
    store: &MongoSandboxStore,
    current: &Agent,
    mutation: &AgentConversationMutation,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    let binding = require_receipt(store, current, session).await?;
    let next = mutation
        .agent
        .conversation
        .first_message
        .as_ref()
        .and_then(|first| first.common_binding.as_ref());
    if binding.as_ref() != next {
        return Err(SandboxStoreError::Conflict);
    }
    if let Some(binding) = binding {
        binding
            .require_agent(&mutation.agent)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        require_command_receipt(store, current, session).await?;
        let before = current
            .conversation
            .first_message
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let after = mutation
            .agent
            .conversation
            .first_message
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        if before.attempt != after.attempt && mutation.trigger_execution_fence.is_none() {
            return Err(SandboxStoreError::Conflict);
        }
    } else if mutation.trigger_execution_fence.is_some() {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

async fn require_ready(
    store: &MongoSandboxStore,
    supplied: &AdmittedAgentCommand,
    current: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    let now = layer2_core::now_rfc3339();
    if current != supplied || !current.awaiting_claim(&now) || current.queue_expired(&now) {
        return Err(SandboxStoreError::Conflict);
    }
    let agent_id = current.receipt.envelope.agent_id.as_str();
    let row = store
        .agent_command_orders
        .find_one(doc! {"_id": agent_id})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::Conflict)?;
    let lane = super::super::agent_commands::read_command_lane(&row, agent_id)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    if lane.lease.as_ref().is_some_and(|lease| lease.live_at(&now))
        || current.receipt.agent_order > lane.last_order
        || current
            .work
            .lease
            .as_ref()
            .is_some_and(|expired| lane.next_fence <= expired.claim.fence)
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}
