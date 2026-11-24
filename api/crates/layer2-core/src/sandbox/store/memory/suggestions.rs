use super::*;
use crate::sandbox::suggestions::*;
use crate::sandbox::{agent_io::AnswerContext, AgentReadError};

pub(super) fn enqueue(state: &mut State, id: &AgentId) {
    let Some(logs) = state.agent_logs.get(id) else {
        return;
    };
    let Some(AgentLogEntry {
        kind: AgentLogKind::TurnCompleted { turn_sequence },
        ..
    }) = logs.last()
    else {
        return;
    };
    let Some(agent) = state.agents.get(id) else {
        return;
    };
    let Some(sandbox) = state.sandboxes.get(&agent.sandbox_id) else {
        return;
    };
    if *turn_sequence == 0
        || *turn_sequence > logs.len() as u64
        || logs.len() as u64 - turn_sequence + 1 > MAX_CONTEXT_EVENTS as u64
    {
        return;
    }
    if let Some(work) = work_from_turn(agent, sandbox, &logs[*turn_sequence as usize - 1..]) {
        state.suggestions.entry(work.id.clone()).or_insert(work);
    }
}

#[async_trait]
impl SuggestionStore for InMemorySandboxStore {
    async fn read_suggestions(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        scope: AnswerContext,
    ) -> Result<SuggestionReading, AgentReadError> {
        let state = self.lock();
        let work = state.suggestions.values().find(|work| {
            &work.owner == owner
                && &work.agent_id == agent
                && work.scope == scope
                && work.strategy_version == STRATEGY_VERSION
        });
        Ok(match work {
            Some(work) => work.reading(current(&state, work)),
            None => SuggestionReading {
                status: SuggestionStatus::Unavailable,
                set: None,
                observation: None,
            },
        })
    }

    async fn claim_suggestions(
        &self,
        now_ms: u64,
    ) -> Result<Option<SuggestionWork>, AgentReadError> {
        let mut state = self.lock();
        let id = state
            .suggestions
            .values()
            .filter(|work| {
                work.status == SuggestionStatus::Pending
                    || (work.status == SuggestionStatus::Running
                        && work.deadline_ms.is_some_and(|deadline| deadline <= now_ms))
            })
            .min_by_key(|work| (work.created_at_ms, &work.id))
            .map(|work| work.id.clone());
        let Some(id) = id else {
            return Ok(None);
        };
        let valid = current(&state, &state.suggestions[&id]);
        let work = state.suggestions.get_mut(&id).unwrap();
        Ok(work.claim(valid, now_ms).then(|| work.clone()))
    }

    async fn complete_suggestions(
        &self,
        claim: &SuggestionWork,
        completion: SuggestionCompletion,
    ) -> Result<(), AgentReadError> {
        let mut state = self.lock();
        let valid = current(&state, claim);
        let work = state
            .suggestions
            .get_mut(&claim.id)
            .ok_or(AgentReadError::NotFound)?;
        if work.status == SuggestionStatus::Running && work.started_at_ms == claim.started_at_ms {
            work.complete(valid, completion);
        }
        Ok(())
    }
}

fn current(state: &State, work: &SuggestionWork) -> bool {
    let Some(agent) = state.agents.get(&work.agent_id) else {
        return false;
    };
    let Some(sandbox) = state.sandboxes.get(&agent.sandbox_id) else {
        return false;
    };
    let logs = state
        .agent_logs
        .get(&agent.id)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let tail = logs
        .get(work.completed_sequence as usize..)
        .unwrap_or_default();
    let newer = state.agent_commands.values().any(|command| {
        command.receipt.envelope.agent_id == agent.id
            && command.receipt.event_cursor >= work.scope.answer_sequence
            && matches!(
                command.receipt.envelope.command,
                crate::sandbox::agent_io::AgentCommand::Message { .. }
            )
    });
    work_is_current(work, agent, sandbox, tail, newer)
}
