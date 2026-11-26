//! A tool rebind is admitted only from the immutable claimed empty-session receipt.
use crate::store::agent_admission::helper_error::HelperError;
use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::sandbox::store::AgentProvenanceSnapshot;
#[cfg(test)]
use layer2_core::sandbox::SandboxStoreError;
use layer2_core::sandbox::{AgentOrigin, AgentStatus, AgentWorkStep};
use layer3_adapter::AgentOpenRequest;
use mongodb::ClientSession;

#[cfg(test)]
pub(in crate::store) async fn prepared_conversation(
    store: &MongoSandboxStore,
    snapshot: &AgentProvenanceSnapshot,
    request: &AgentOpenRequest,
    session: &mut ClientSession,
) -> Result<Option<String>, SandboxStoreError> {
    prepared_conversation_transaction(store, snapshot, request, session)
        .await
        .map_err(|error| error.into_store_error(super::unavailable))
}

pub(in crate::store) async fn prepared_conversation_transaction(
    store: &MongoSandboxStore,
    snapshot: &AgentProvenanceSnapshot,
    request: &AgentOpenRequest,
    session: &mut ClientSession,
) -> Result<Option<String>, HelperError> {
    let lineage = &snapshot.resource.lineage;
    let Some(record) =
        super::adoption::claimed_transaction(store, &lineage.sandbox, session).await?
    else {
        return Ok(None);
    };
    let owned = record.ownership.as_ref().expect("validated claimed record");
    let workspace = &record.payload.workspaces[0];
    let prepared = &workspace.agents[0];
    let fail = || super::model::corrupt("prepared tool handoff does not match durable adoption");
    let agent = lineage.agent.as_ref().ok_or_else(fail)?;
    // After publication ordinary confirmed capability receipts own reconnects.
    if agent.status != AgentStatus::Starting || agent.ready_at.is_some() {
        return Ok(None);
    }
    let tree = lineage.worktree.as_ref().ok_or_else(fail)?;
    // Preparation belongs to its one adopted Agent, not every later Agent in
    // the same Sandbox. Ordinary children enroll their own fresh tool binding.
    // A different identity must never borrow any part of the prepared handoff.
    if agent.id != prepared.id {
        if agent.work.warm_conversation_id.is_some()
            || agent.worktree_id == prepared.worktree_id
            || tree.id == prepared.worktree_id
            || agent.origin
                == (AgentOrigin::Lifecycle {
                    operation_id: owned.operation_id.clone(),
                })
        {
            return Err(fail().into());
        }
        return Ok(None);
    }
    let conversation = prepared.conversation_id.as_ref().ok_or_else(fail)?;
    let expected = AgentOpenRequest::new(
        format!("agent:{}:open:v1", prepared.id),
        crate::sandbox::runtime_options(agent, &lineage.sandbox, tree),
        None,
    );
    if owned.agent.as_ref() != Some(&agent.profile)
        || agent.id != prepared.id
        || agent.worktree_id != prepared.worktree_id
        || agent.workspace_id != workspace.id
        || tree.id != prepared.worktree_id
        || tree.base_commit != workspace.resolved_commit.clone().unwrap_or_default()
        || tree.path != "/home/cma/workspace/.worktrees/wt-1"
        || lineage
            .workspace
            .as_ref()
            .is_none_or(|w| w.id != workspace.id || w.profile != owned.workspace)
        || agent.origin
            != (AgentOrigin::Lifecycle {
                operation_id: owned.operation_id.clone(),
            })
        || agent.work.warm_conversation_id.as_ref() != Some(conversation)
        || !matches!(
            agent.work.step,
            AgentWorkStep::OpenConversation | AgentWorkStep::PublishReady
        )
        || agent.conversation.active_turn.is_some()
        || agent.conversation.operation.is_some()
        || agent
            .conversation
            .conversation_id
            .as_ref()
            .is_some_and(|id| id != conversation)
        || request != &expected
    {
        return Err(fail().into());
    }
    Ok(Some(conversation.clone()))
}
