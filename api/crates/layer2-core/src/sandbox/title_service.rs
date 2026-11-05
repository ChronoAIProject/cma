//! Claim-before-call orchestration for write-once agent titles.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::ids::AgentId;
use crate::now_rfc3339;

use super::store::{Page, PageRequest};
use super::{
    AgentTitle, AgentTitleClaimMode, AgentTitleClaimRequest, AgentTitleGenerationContext,
    AgentTitleGenerationRequest, AgentTitleGenerator, AgentTitleSource, AgentTitleStore,
    AgentTitleStoreError, SetAgentTitleOutcome, StoredAgentTitle, AGENT_TITLE_ATTEMPTS,
    AGENT_TITLE_CALL_TIMEOUT_SECONDS,
};

/// Content-free result of settling one eligible title candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTitleOutcome {
    Generated,
    Fallback,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentTitleServiceError {
    #[error("invalid agent title coordination holder")]
    InvalidHolder,
    #[error("agent title store returned inconsistent first-turn evidence")]
    InconsistentEvidence,
    #[error("agent title generation is temporarily unavailable")]
    GenerationUnavailable,
    #[error(transparent)]
    Store(#[from] AgentTitleStoreError),
}

pub struct AgentTitleService {
    store: Arc<dyn AgentTitleStore>,
    coordination_holder: String,
}

impl AgentTitleService {
    pub fn new<S>(
        store: Arc<S>,
        coordination_holder: impl Into<String>,
    ) -> Result<Arc<Self>, AgentTitleServiceError>
    where
        S: AgentTitleStore + 'static,
    {
        let coordination_holder = coordination_holder.into();
        if coordination_holder.trim().is_empty() || coordination_holder.len() > 256 {
            return Err(AgentTitleServiceError::InvalidHolder);
        }
        Ok(Arc::new(Self {
            store,
            coordination_holder,
        }))
    }

    pub async fn candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentId>, AgentTitleServiceError> {
        self.store
            .agent_title_candidates(page)
            .await
            .map_err(Into::into)
    }

    pub async fn title(
        &self,
        id: &AgentId,
    ) -> Result<Option<StoredAgentTitle>, AgentTitleServiceError> {
        self.store.agent_title(id).await.map_err(Into::into)
    }

    pub async fn set_user_title(
        &self,
        id: &AgentId,
        title: &AgentTitle,
    ) -> Result<SetAgentTitleOutcome, AgentTitleServiceError> {
        self.store
            .set_user_agent_title(id, title, now_rfc3339())
            .await
            .map_err(Into::into)
    }

    /// Claim and settle one title after its first durable completed turn.
    pub async fn generate_agent_title(
        &self,
        id: &AgentId,
        context: Option<&AgentTitleGenerationContext>,
        generator: Option<&dyn AgentTitleGenerator>,
    ) -> Result<AgentTitleOutcome, AgentTitleServiceError> {
        let request = AgentTitleClaimRequest::new(id.clone(), &self.coordination_holder);
        let Some(claim) = self
            .store
            .acquire_agent_title_claim(&request, SystemTime::now())
            .await?
        else {
            return Ok(AgentTitleOutcome::Skipped);
        };
        let transcript = self
            .store
            .agent_title_transcript(id)
            .await?
            .ok_or(AgentTitleServiceError::InconsistentEvidence)?;
        let fallback = AgentTitle::fallback(Some(transcript.owner_message()));

        let generated = if claim.mode() == AgentTitleClaimMode::Generate {
            match (context, generator) {
                (Some(context), Some(generator)) => {
                    generate_title(generator, context, &transcript).await
                }
                _ => None,
            }
        } else {
            None
        };
        // Retain the existing display title and retry through the durable lease.
        // A temporary model failure must not permanently settle the first prompt.
        if generated.is_none() && generator.is_some() && context.is_some() {
            if self
                .store
                .agent_title(id)
                .await?
                .is_some_and(|stored| stored.source != AgentTitleSource::Fallback)
            {
                return Ok(AgentTitleOutcome::Skipped);
            }
            return Err(AgentTitleServiceError::GenerationUnavailable);
        }
        let (title, source, outcome) = match generated {
            Some(title) => (
                title,
                AgentTitleSource::Generated,
                AgentTitleOutcome::Generated,
            ),
            None => (
                fallback,
                AgentTitleSource::Fallback,
                AgentTitleOutcome::Fallback,
            ),
        };

        if !self
            .store
            .complete_agent_title(&claim, &title, source, now_rfc3339())
            .await?
        {
            return Ok(AgentTitleOutcome::Skipped);
        }
        Ok(outcome)
    }
}

async fn generate_title(
    generator: &dyn AgentTitleGenerator,
    context: &AgentTitleGenerationContext,
    transcript: &super::AgentTitleTranscript,
) -> Option<AgentTitle> {
    let request = AgentTitleGenerationRequest::new(context, transcript);
    for _ in 0..AGENT_TITLE_ATTEMPTS {
        let generated = tokio::time::timeout(
            Duration::from_secs(AGENT_TITLE_CALL_TIMEOUT_SECONDS),
            generator.generate(request.clone()),
        )
        .await;
        if let Ok(Ok(response)) = generated {
            if let Some(title) = AgentTitle::sanitize(response.as_str()) {
                return Some(title);
            }
        }
    }
    None
}
