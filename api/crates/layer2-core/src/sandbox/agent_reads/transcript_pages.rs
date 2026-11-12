use super::*;
use crate::resource_access::CallerContext;
use crate::sandbox::{AgentTranscriptPage, AgentTranscriptPageRequest};

impl AgentReadService {
    pub async fn transcript_page(
        &self,
        caller: &Caller,
        id: &AgentId,
        request: &AgentTranscriptPageRequest,
    ) -> Result<AgentTranscriptPage, AgentReadError> {
        require(caller)?;
        request.validate()?;
        let (lineage, held) = self
            .owner_projection_authority(caller, id, crate::RuntimeCapability::Conversation)
            .await?;
        let result = self.read_transcript_page(&lineage.agent, request).await?;
        self.finish_owner_projection(
            caller,
            id,
            &lineage,
            held,
            crate::RuntimeCapability::Conversation,
        )
        .await?;
        Ok(result)
    }

    pub(super) async fn owner_projection_authority(
        &self,
        caller: &Caller,
        id: &AgentId,
        capability: crate::RuntimeCapability,
    ) -> Result<
        (
            AgentLineage,
            Option<crate::resource_access::AuthorizedResource>,
        ),
        AgentReadError,
    > {
        require(caller)?;
        let held = if let Some(access) = &self.access {
            Some(
                access
                    .authorize_personal(
                        caller,
                        &ResourceTarget::Agent {
                            agent_id: id.clone(),
                        },
                        ResourceCapability::ViewHistory,
                    )
                    .await
                    .map_err(authorized::access_error)?,
            )
        } else {
            None
        };
        let lineage = match &held {
            Some(held) => authorized::lineage(held.snapshot.lineage.clone())?,
            None => self.owned_lineage(caller, id).await?,
        };
        require_granted(&lineage.agent, capability)?;
        Ok((lineage, held))
    }

    pub(super) async fn finish_owner_projection(
        &self,
        caller: &Caller,
        id: &AgentId,
        lineage: &AgentLineage,
        held: Option<crate::resource_access::AuthorizedResource>,
        capability: crate::RuntimeCapability,
    ) -> Result<(), AgentReadError> {
        if let Some(held) = held {
            self.finish_read(&held, id, ResourceCapability::ViewHistory)
                .await?;
        } else {
            let current = self.owned_lineage(caller, id).await?;
            require_granted(&current.agent, capability)?;
            if current.agent != lineage.agent {
                return Err(AgentReadError::Unavailable(
                    "transcript source changed".into(),
                ));
            }
        }
        Ok(())
    }

    pub async fn transcript_page_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        request: &AgentTranscriptPageRequest,
    ) -> Result<AgentTranscriptPage, AgentReadError> {
        request.validate()?;
        let held = self
            .authorized(context, credential, id, ResourceCapability::ViewHistory)
            .await?;
        let agent = held
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or_else(|| corrupt("Agent is absent"))?;
        require_granted(agent, crate::RuntimeCapability::Conversation)?;
        let mut result = self.read_transcript_page(agent, request).await?;
        if !authorized::is_personal(&held, context) {
            visibility::shared_transcript(&mut result.transcript);
        }
        self.finish_read(&held, id, ResourceCapability::ViewHistory)
            .await?;
        Ok(result)
    }

    async fn read_transcript_page(
        &self,
        agent: &Agent,
        request: &AgentTranscriptPageRequest,
    ) -> Result<AgentTranscriptPage, AgentReadError> {
        if request
            .through
            .is_some_and(|seq| seq != agent.conversation.last_seq)
        {
            return Err(AgentReadError::Unavailable(
                "transcript source changed".into(),
            ));
        }
        if let Some(page) = self
            .store
            .agent_log_transcript_page(agent, request)
            .await
            .map_err(store_error)?
        {
            return Ok(page);
        }
        let transcript = self.read_transcript(agent).await?;
        legacy_page(transcript, request)
    }
}

fn legacy_page(
    mut transcript: AgentTranscript,
    request: &AgentTranscriptPageRequest,
) -> Result<AgentTranscriptPage, AgentReadError> {
    let index = match request.turn {
        Some(sequence) => transcript
            .turns
            .iter()
            .position(|turn| turn.turn_sequence == sequence)
            .ok_or(AgentReadError::InvalidPage)?,
        None if transcript.turns.is_empty() => {
            return Ok(AgentTranscriptPage {
                transcript,
                next: None,
            })
        }
        None => 0,
    };
    let mut turn = transcript.turns[index].clone();
    let after = request.after.unwrap_or(turn.turn_sequence - 1);
    let end = transcript
        .turns
        .get(index + 1)
        .map_or(transcript.through_seq, |turn| turn.turn_sequence - 1);
    if after < turn.turn_sequence - 1 || after >= end {
        return Err(AgentReadError::InvalidPage);
    }
    let through = end.min(after + u64::from(request.limit));
    turn.messages.retain(|message| {
        message.sequence <= through
            && (message.sequence > after
                || (after == turn.turn_sequence - 1 && message.sequence < turn.turn_sequence))
    });
    turn.agent_messages = turn
        .messages
        .iter()
        .filter(|message| message.role == crate::sandbox::AgentTranscriptRole::Agent)
        .map(|message| message.content.clone())
        .collect();
    let next = if through < end {
        Some((turn.turn_sequence, through))
    } else {
        transcript
            .turns
            .get(index + 1)
            .map(|next| (next.turn_sequence, next.turn_sequence - 1))
    }
    .map(|(turn, after)| AgentTranscriptPageRequest {
        turn: Some(turn),
        after: Some(after),
        through: Some(transcript.through_seq),
        limit: request.limit,
    });
    transcript.turns = vec![turn];
    Ok(AgentTranscriptPage { transcript, next })
}
