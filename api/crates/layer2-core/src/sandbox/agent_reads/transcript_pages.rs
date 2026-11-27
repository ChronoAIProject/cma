use super::*;
use crate::resource_access::CallerContext;
use crate::sandbox::{AgentTranscriptPage, AgentTranscriptPageRequest};

impl AgentReadService {
    pub async fn transcript(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentTranscript, AgentReadError> {
        let (lineage, held) = self
            .owner_projection_authority(caller, id, crate::RuntimeCapability::Conversation)
            .await?;
        let mut transcript = self.read_transcript(&lineage.agent).await?;
        crate::sandbox::sanitize_agent_transcript(&mut transcript);
        self.finish_owner_projection(
            caller,
            id,
            &lineage,
            held,
            crate::RuntimeCapability::Conversation,
        )
        .await?;
        Ok(transcript)
    }

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
        let mut result = self.read_transcript_page(&lineage.agent, request).await?;
        crate::sandbox::sanitize_agent_transcript(&mut result.transcript);
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
        crate::sandbox::sanitize_agent_transcript(&mut result.transcript);
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
        transcript.into_page(request)
    }
}
