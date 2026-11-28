use std::borrow::Cow;

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

pub(super) async fn terminal<'a>(
    source: Option<&dyn SpendSource>,
    agent: &Agent,
    update: Cow<'a, ConversationUpdate>,
) -> Result<Cow<'a, ConversationUpdate>, AgentConversationError> {
    let Some(generation) = agent.conversation.runtime_generation else {
        return Ok(update);
    };
    let terminal = match update.as_ref() {
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
    Ok(Cow::Owned(match update.as_ref() {
        ConversationUpdate::TurnEndedForPrompt { prompt_id, .. }
        | ConversationUpdate::FailedForPrompt { prompt_id, .. } => {
            ConversationUpdate::FailedForPrompt {
                prompt_id: prompt_id.clone(),
                message,
                capability,
            }
        }
        _ => ConversationUpdate::Failed {
            message,
            capability,
        },
    }))
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
        Self::with_live_budget(store, writer_policy, spend_source, false)
    }

    pub fn with_live_budget<S>(
        store: Arc<S>,
        writer_policy: Arc<super::super::task_experience::TaskExperiencePolicy>,
        spend_source: Option<Arc<dyn crate::budget::SpendSource>>,
        bounded_live_fragments: bool,
    ) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Self::with_ingress_budget(
            store,
            writer_policy,
            spend_source,
            bounded_live_fragments,
            false,
        )
    }

    pub fn with_ingress_budget<S>(
        store: Arc<S>,
        writer_policy: Arc<super::super::task_experience::TaskExperiencePolicy>,
        spend_source: Option<Arc<dyn crate::budget::SpendSource>>,
        bounded_live_fragments: bool,
        coordinated_ingress: bool,
    ) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Self::with_output_policy(
            store,
            writer_policy,
            spend_source,
            bounded_live_fragments,
            coordinated_ingress,
            None,
        )
    }

    pub fn with_output_policy<S>(
        store: Arc<S>,
        writer_policy: Arc<super::super::task_experience::TaskExperiencePolicy>,
        spend_source: Option<Arc<dyn crate::budget::SpendSource>>,
        bounded_live_fragments: bool,
        coordinated_ingress: bool,
        coalesce_policy: Option<super::AgentOutputCoalescePolicy>,
    ) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Self::with_incremental_output_policy(
            store,
            writer_policy,
            spend_source,
            bounded_live_fragments,
            coordinated_ingress,
            coalesce_policy,
            false,
        )
    }

    pub fn with_incremental_output_policy<S>(
        store: Arc<S>,
        writer_policy: Arc<super::super::task_experience::TaskExperiencePolicy>,
        spend_source: Option<Arc<dyn crate::budget::SpendSource>>,
        bounded_live_fragments: bool,
        coordinated_ingress: bool,
        coalesce_policy: Option<super::AgentOutputCoalescePolicy>,
        incremental_text: bool,
    ) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Arc::new(Self {
            live_source: std::sync::OnceLock::new(),
            store,
            writer_policy,
            spend_source,
            coalesce_policy,
            incremental_text,
            live: if bounded_live_fragments {
                live::AgentLiveHub::with_budget(if coordinated_ingress {
                    super::live_budget::LiveBudget::process_coordinated()
                } else {
                    super::live_budget::LiveBudget::process()
                })
            } else {
                live::AgentLiveHub::default()
            },
        })
    }
}
