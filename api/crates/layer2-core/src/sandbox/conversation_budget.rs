use crate::budget::{BudgetStop, SpendSource};
use crate::sandbox::{Agent, AgentConversationError};
use layer3_adapter::ConversationUpdate;

pub(super) async fn stop(
    source: Option<&dyn SpendSource>,
    id: &crate::AgentId,
    runtime_generation: u64,
) -> Result<Option<BudgetStop>, AgentConversationError> {
    let Some(source) = source else {
        return Ok(None);
    };
    let now = crate::seconds_from_rfc3339(&crate::now_rfc3339())
        .ok_or_else(|| AgentConversationError::Unavailable("budget clock is unavailable".into()))?;
    source
        .stop_requested(id, runtime_generation, now)
        .await
        .map_err(|_| AgentConversationError::Unavailable("budget accounting is unavailable".into()))
}

pub(super) async fn terminal(
    source: Option<&dyn SpendSource>,
    agent: &Agent,
    update: ConversationUpdate,
) -> Result<ConversationUpdate, AgentConversationError> {
    let Some(generation) = agent.conversation.runtime_generation else {
        return Ok(update);
    };
    let terminal = match &update {
        ConversationUpdate::TurnEnded(_)
        | ConversationUpdate::Failed { .. }
        | ConversationUpdate::TurnEndedForPrompt { .. }
        | ConversationUpdate::FailedForPrompt { .. } => true,
        ConversationUpdate::WithoutPromptEvidence(_) => false,
        _ => false,
    };
    if !terminal {
        return Ok(update);
    }
    let Some(stop) = stop(source, &agent.id, generation).await? else {
        return Ok(update);
    };
    if stop.agent_id != agent.id || stop.runtime_generation != generation {
        return Err(AgentConversationError::Unavailable(
            "budget stop scope is stale".into(),
        ));
    }
    if agent.conversation.active_turn != Some(stop.turn_sequence) {
        return Ok(update);
    }
    let message = stop.reason.guidance().to_owned();
    let capability = Some(stop.reason.code().to_owned());
    Ok(match update {
        ConversationUpdate::TurnEndedForPrompt { prompt_id, .. }
        | ConversationUpdate::FailedForPrompt { prompt_id, .. } => {
            ConversationUpdate::FailedForPrompt {
                prompt_id,
                message,
                capability,
            }
        }
        _ => ConversationUpdate::Failed {
            message,
            capability,
        },
    })
}

use super::{live, AgentConversationUpdateStore, Arc, DurableAgentUpdateRecorder};

impl DurableAgentUpdateRecorder {
    pub fn new<S>(store: Arc<S>) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Self::with_writer_policy(store, Arc::default())
    }

    pub fn with_writer_policy<S>(
        store: Arc<S>,
        writer_policy: Arc<super::super::task_experience::TaskExperiencePolicy>,
    ) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Self::with_spend_source(store, writer_policy, None)
    }

    pub fn with_spend_source<S>(
        store: Arc<S>,
        writer_policy: Arc<super::super::task_experience::TaskExperiencePolicy>,
        spend_source: Option<Arc<dyn crate::budget::SpendSource>>,
    ) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Arc::new(Self {
            store,
            writer_policy,
            spend_source,
            live: live::AgentLiveHub::default(),
        })
    }
}
