use super::*;
use crate::resource_access::CallerContext;
use crate::sandbox::{
    AgentLogKind, AgentTranscriptMessage, AgentTranscriptRole, AgentTranscriptTurn,
    AgentTurnOutcome,
};

pub struct AgentFinalAnswer {
    pub turn_sequence: u64,
    pub message: AgentTranscriptMessage,
    authority: AnswerAuthority,
}

enum AnswerAuthority {
    Shared(Box<AgentEventStreamAuthorization>),
    Personal(Caller, Box<AgentLineage>),
}

impl AgentReadService {
    pub async fn final_answer(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        id: &AgentId,
        sequence: u64,
    ) -> Result<AgentFinalAnswer, AgentReadError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            Box::pin(async {
                let (authority, agent) = if let Some(context) = context {
                    if &context.caller != caller {
                        return Err(AgentReadError::NotFound);
                    }
                    let held =
                        Box::pin(self.authorize_event_stream(context, credential, id)).await?;
                    let agent = Box::pin(self.current_event_stream_snapshot(&held))
                        .await?
                        .lineage
                        .agent
                        .ok_or(AgentReadError::NotFound)?;
                    (AnswerAuthority::Shared(Box::new(held)), agent)
                } else {
                    require(caller)?;
                    let lineage = self.owned_lineage(caller, id).await?;
                    (
                        AnswerAuthority::Personal(caller.clone(), Box::new(lineage.clone())),
                        lineage.agent,
                    )
                };
                require_granted(&agent, crate::RuntimeCapability::Conversation)?;
                match self
                    .store
                    .agent_log_final_answer(&agent, sequence)
                    .await
                    .map_err(log_store_error)?
                {
                    crate::sandbox::AgentLogAnswerLookup::Legacy => {}
                    crate::sandbox::AgentLogAnswerLookup::NotFinal => {
                        return Err(AgentReadError::NotFound)
                    }
                    crate::sandbox::AgentLogAnswerLookup::Answer(evidence) => {
                        let answer = projected_answer(&agent, sequence, *evidence, authority)?;
                        Box::pin(self.validate_final_answer(&answer)).await?;
                        return Ok(answer);
                    }
                }
                let mut budget = 4 * 1024 * 1024;
                let entry = self.answer_entry(&agent, sequence, &mut budget).await?;
                if !matches!(entry.kind, AgentLogKind::AgentMessage { .. }) {
                    return Err(AgentReadError::NotFound);
                }
                let mut terminal = None;
                for next in sequence.saturating_add(1)
                    ..=agent
                        .conversation
                        .last_seq
                        .min(sequence.saturating_add(128))
                {
                    let entry = self.answer_entry(&agent, next, &mut budget).await?;
                    match entry.kind {
                        AgentLogKind::TurnCompleted { turn_sequence } => {
                            terminal = Some((turn_sequence, next, entry.appended_at));
                            break;
                        }
                        AgentLogKind::TurnStarted { .. }
                        | AgentLogKind::ConversationGap { .. }
                        | AgentLogKind::TurnFailed { .. }
                        | AgentLogKind::TurnInterrupted { .. }
                        | AgentLogKind::ConversationOpened { .. }
                        | AgentLogKind::RuntimeGenerationRetired { .. }
                        | AgentLogKind::Created { .. } => {
                            return Err(AgentReadError::NotFound);
                        }
                        _ => {}
                    }
                }
                let (turn_sequence, end, completed_at) = terminal.ok_or_else(|| {
                    if agent.conversation.last_seq > sequence.saturating_add(128) {
                        AgentReadError::Unavailable("answer exceeds the forward read bound".into())
                    } else {
                        AgentReadError::NotFound
                    }
                })?;
                if turn_sequence == 0 || turn_sequence >= sequence {
                    return Err(AgentReadError::NotFound);
                }
                if end - turn_sequence > 512 {
                    return Err(AgentReadError::Unavailable(
                        "answer exceeds the turn read bound".into(),
                    ));
                }
                let start = self
                    .answer_entry(&agent, turn_sequence, &mut budget)
                    .await?;
                let AgentLogKind::TurnStarted {
                    turn_sequence: declared,
                    origin_key,
                    ..
                } = start.kind
                else {
                    return Err(AgentReadError::NotFound);
                };
                if declared != turn_sequence {
                    return Err(AgentReadError::NotFound);
                }
                let mut turn = AgentTranscriptTurn {
                    turn_sequence,
                    started_at: start.appended_at,
                    origin_key,
                    owner_message: String::new(),
                    agent_messages: Vec::new(),
                    messages: Vec::new(),
                    metadata: Default::default(),
                    outcome: AgentTurnOutcome::Completed { at: completed_at },
                };
                for current in turn_sequence + 1..end {
                    let entry = self.answer_entry(&agent, current, &mut budget).await?;
                    let (role, content) = match entry.kind {
                        AgentLogKind::AgentMessage { content } => {
                            (AgentTranscriptRole::Agent, content)
                        }
                        AgentLogKind::OwnerMessage { content } => {
                            (AgentTranscriptRole::Owner, content)
                        }
                        AgentLogKind::TurnStarted { .. }
                        | AgentLogKind::ConversationGap { .. }
                        | AgentLogKind::TurnCompleted { .. }
                        | AgentLogKind::TurnFailed { .. }
                        | AgentLogKind::TurnInterrupted { .. }
                        | AgentLogKind::ConversationOpened { .. }
                        | AgentLogKind::RuntimeGenerationRetired { .. }
                        | AgentLogKind::Created { .. } => return Err(AgentReadError::NotFound),
                        _ => continue,
                    };
                    turn.messages.push(AgentTranscriptMessage {
                        sequence: current,
                        role,
                        content,
                        appended_at: entry.appended_at,
                    });
                }
                let message = turn
                    .final_answer()
                    .filter(|message| message.sequence == sequence)
                    .ok_or(AgentReadError::NotFound)?
                    .clone();
                let answer = AgentFinalAnswer {
                    turn_sequence,
                    message,
                    authority,
                };
                Box::pin(self.validate_final_answer(&answer)).await?;
                Ok(answer)
            }),
        )
        .await
        .map_err(|_| AgentReadError::Unavailable("answer read timed out".into()))?
    }

    pub async fn validate_final_answer(
        &self,
        answer: &AgentFinalAnswer,
    ) -> Result<(), AgentReadError> {
        match &answer.authority {
            AnswerAuthority::Shared(held) => Box::pin(self.validate_event_stream(held)).await,
            AnswerAuthority::Personal(caller, original) => {
                let current = self.owned_lineage(caller, &original.agent.id).await?;
                if current.agent.created_at != original.agent.created_at
                    || current.sandbox.id != original.sandbox.id
                    || current.sandbox.created_at != original.sandbox.created_at
                    || current.workspace.id != original.workspace.id
                    || current.workspace.created_at != original.workspace.created_at
                    || current.worktree.id != original.worktree.id
                {
                    return Err(AgentReadError::NotFound);
                }
                Ok(())
            }
        }
    }

    async fn answer_entry(
        &self,
        agent: &Agent,
        sequence: u64,
        budget: &mut usize,
    ) -> Result<AgentLogEntry, AgentReadError> {
        if sequence == 0 || sequence > agent.conversation.last_seq {
            return Err(AgentReadError::NotFound);
        }
        let entry = self
            .store
            .agent_log_entry(&agent.id, sequence)
            .await
            .map_err(log_store_error)?
            .ok_or_else(|| corrupt("answer log contains a gap"))?;
        if entry.seq != sequence
            || entry.agent_id != agent.id
            || crate::seconds_from_rfc3339(&entry.appended_at).is_none()
        {
            return Err(corrupt("answer log identity is invalid"));
        }
        let size = serde_json::to_vec(&entry)
            .map_err(|_| corrupt("invalid answer log"))?
            .len();
        *budget = budget.checked_sub(size).ok_or_else(|| {
            AgentReadError::Unavailable("answer exceeds the read byte bound".into())
        })?;
        Ok(entry)
    }
}

fn projected_answer(
    agent: &Agent,
    sequence: u64,
    evidence: crate::sandbox::AgentLogAnswerEvidence,
    authority: AnswerAuthority,
) -> Result<AgentFinalAnswer, AgentReadError> {
    let facts = &evidence.facts;
    facts
        .validate(agent.conversation.last_seq)
        .map_err(AgentReadError::Corrupt)?;
    if facts.final_answer_sequence() != Some(sequence)
        || evidence.message.seq != sequence
        || evidence.start.seq != facts.turn_sequence
        || evidence.start.appended_at != facts.started_at
        || facts.end_sequence != Some(evidence.terminal.seq)
        || !matches!(&facts.outcome, AgentTurnOutcome::Completed { at } if at == &evidence.terminal.appended_at)
        || !matches!(&evidence.start.kind, AgentLogKind::TurnStarted { turn_sequence, origin_key, .. }
            if *turn_sequence == facts.turn_sequence && origin_key == &facts.origin_key)
        || evidence.terminal.kind
            != (AgentLogKind::TurnCompleted {
                turn_sequence: facts.turn_sequence,
            })
    {
        return Err(corrupt(
            "answer evidence disagrees with its original source",
        ));
    }
    let mut bytes = 0;
    for entry in [&evidence.start, &evidence.message, &evidence.terminal] {
        if entry.agent_id != agent.id
            || entry.seq == 0
            || entry.seq > agent.conversation.last_seq
            || crate::seconds_from_rfc3339(&entry.appended_at).is_none()
        {
            return Err(corrupt("answer source identity is invalid"));
        }
        bytes += serde_json::to_vec(entry)
            .map_err(|_| corrupt("invalid answer source"))?
            .len();
        if bytes > 4 * 1024 * 1024 {
            return Err(AgentReadError::Unavailable(
                "answer exceeds the read byte bound".into(),
            ));
        }
    }
    let AgentLogKind::AgentMessage { content } = evidence.message.kind else {
        return Err(corrupt(
            "answer evidence does not reference an Agent message",
        ));
    };
    if content.trim().is_empty() {
        return Err(corrupt("answer evidence references an empty message"));
    }
    Ok(AgentFinalAnswer {
        turn_sequence: facts.turn_sequence,
        message: AgentTranscriptMessage {
            sequence,
            role: AgentTranscriptRole::Agent,
            content,
            appended_at: evidence.message.appended_at,
        },
        authority,
    })
}
