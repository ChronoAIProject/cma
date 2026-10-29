//! Retain historical rows while proving the retired hierarchy has no work.
use super::*;
use layer2_core::sandbox::{
    binding::BindingConversationState, Agent, AgentWorkStep, WorktreeStatus,
};
use std::collections::{BTreeMap, BTreeSet};

impl MongoBindingCredentials {
    pub(super) async fn require_terminal_children(
        &self,
        binding: &ProviderBinding,
        session: &mut ClientSession,
    ) -> Result<(), CoreError> {
        let mut ids = BTreeSet::new();
        let mut retained = Vec::new();
        let mut archives = BTreeMap::new();
        let mut conversations = self
            .store
            .bindings
            .conversations
            .find(doc! {"$or":[{"binding_id":&binding.binding_id},{"record.binding_id":&binding.binding_id}]})
            .limit(1025)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let mut count = 0;
        while let Some(document) = conversations
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            let conversation = read_conversation(document).map_err(unavailable)?;
            count += 1;
            if count > 1024
                || conversation.binding_id != binding.binding_id
                || conversation.owner != binding.owner
                || conversation.generation > binding.generation
                || conversation.borrowed
                || !matches!(
                    conversation.state,
                    BindingConversationState::Ready
                        | BindingConversationState::Failed
                        | BindingConversationState::Retired
                )
            {
                return Err(conflict());
            }
            if conversation.generation == binding.generation {
                if conversation.sandbox_id != binding.sandbox_id
                    || conversation.workspace_id != binding.workspace_id
                {
                    return Err(conflict());
                }
                if conversation.state != BindingConversationState::Retired {
                    retained.push(conversation.clone());
                }
            } else if conversation.state != BindingConversationState::Retired {
                // A former generation cannot be written through the generic
                // store. Its immutable migration receipt proves the same child
                // retirement checks already committed. Do not require purged
                // historical resource rows to exist forever.
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    archives.entry(conversation.generation)
                {
                    let document = self
                        .store
                        .bindings
                        .preparations
                        .find_one(doc! {
                            "binding_id":&binding.binding_id,
                            "generation":integer(conversation.generation).map_err(unavailable)?,
                        })
                        .session(&mut *session)
                        .await
                        .map_err(unavailable)?
                        .ok_or_else(conflict)?;
                    let receipt = read_receipt(&document)?;
                    entry.insert(receipt.previous);
                }
                let original = &archives[&conversation.generation];
                if original.binding_id != binding.binding_id
                    || original.owner != binding.owner
                    || original.generation != conversation.generation
                    || original.sandbox_id != conversation.sandbox_id
                    || original.workspace_id != conversation.workspace_id
                {
                    return Err(conflict());
                }
            }
            ids.insert(conversation.agent_id.to_string());
        }
        let mut agents = self
            .store
            .agents
            .find(doc! {"$or":[
                {"sandbox_id":binding.sandbox_id.as_str()},
                {"record.sandbox_id":binding.sandbox_id.as_str()}
            ]})
            .limit(1025)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let mut settled = BTreeMap::new();
        while let Some(document) = agents
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            let agent = crate::store::agents::read_agent(&document).map_err(unavailable)?;
            if settled.len() >= 1024
                || has_cleanup(&document)
                || agent.owner != binding.owner
                || agent.sandbox_id != binding.sandbox_id
                || !settled_agent(&agent)
            {
                return Err(conflict());
            }
            ids.insert(agent.id.to_string());
            if settled.insert(agent.id.clone(), agent).is_some() {
                return Err(conflict());
            }
        }
        for conversation in &retained {
            let agent = settled.get(&conversation.agent_id).ok_or_else(conflict)?;
            if agent.workspace_id != conversation.workspace_id
                || agent.worktree_id != conversation.worktree_id
            {
                return Err(conflict());
            }
        }
        let mut worktrees = self
            .store
            .worktrees
            .find(doc! {"$or":[
                {"sandbox_id":binding.sandbox_id.as_str()},
                {"record.sandbox_id":binding.sandbox_id.as_str()}
            ]})
            .limit(1025)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let mut count = 0;
        let mut retained_worktrees = BTreeMap::new();
        while let Some(document) = worktrees
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            let worktree =
                crate::store::worktrees::read_worktree(&document).map_err(unavailable)?;
            count += 1;
            if count > 1024
                || has_cleanup(&document)
                || worktree.owner != binding.owner
                || worktree.sandbox_id != binding.sandbox_id
                || !matches!(
                    worktree.status,
                    WorktreeStatus::Ready | WorktreeStatus::Failed | WorktreeStatus::Removed
                )
            {
                return Err(conflict());
            }
            if let Some(agent_id) = &worktree.agent_id {
                let agent = settled.get(agent_id).ok_or_else(conflict)?;
                if agent.worktree_id != worktree.id || agent.workspace_id != worktree.workspace_id {
                    return Err(conflict());
                }
            }
            retained_worktrees.insert(worktree.id.clone(), worktree);
        }
        if retained.iter().any(|conversation| {
            retained_worktrees
                .get(&conversation.worktree_id)
                .is_none_or(|worktree| {
                    worktree.workspace_id != conversation.workspace_id
                        || worktree
                            .agent_id
                            .as_ref()
                            .is_some_and(|id| id != &conversation.agent_id)
                })
        }) {
            return Err(conflict());
        }
        let ids: Vec<_> = ids.into_iter().collect();
        if !ids.is_empty() && self.store.agent_commands.find_one(doc! {
            "$and":[
                {"$or":[{"agent_id":{"$in":&ids}}, {"record.receipt.envelope.agent_id":{"$in":&ids}}]},
                {"$or":[{"status":{"$in":["queued","in_progress","waiting_action"]}},
                    {"record.status":{"$in":["queued","in_progress","waiting_action"]}}]},
            ],
        }).session(&mut *session).await.map_err(unavailable)?.is_some() { return Err(conflict()); }
        Ok(())
    }
}

fn has_cleanup(document: &Document) -> bool {
    document.contains_key("channel_reuse_cleanup_execution")
        || document.contains_key("owned_workspace_cleanup_admission_v1")
}

fn settled_agent(agent: &Agent) -> bool {
    // Owner Sandbox retirement stops Agents and retains their history; it does
    // not delete Agents, Worktrees, or binding conversation aliases.
    !agent.status.is_live()
        && agent.work.step == AgentWorkStep::None
        && agent.work.lease.is_none()
        && agent.work.cleanup_debt.is_empty()
        && agent.requested_stop_reason.is_none()
        && agent.conversation.active_turn.is_none()
        && agent.conversation.operation.is_none()
        && agent.conversation.queued_messages.is_empty()
}
