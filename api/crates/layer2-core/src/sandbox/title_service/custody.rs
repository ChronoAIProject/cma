use super::*;
use crate::sandbox::{AgentTitleClaim, AgentTitleEffectIntent, AgentTitleEffectState};
use tokio::time::{timeout_at, Instant};

impl AgentTitleService {
    pub(super) async fn generate_with_custody(
        &self,
        claim: AgentTitleClaim,
        state: AgentTitleEffectState,
        context: Option<&AgentTitleGenerationContext>,
        generator: Option<&dyn AgentTitleGenerator>,
        deadline: Instant,
    ) -> Result<AgentTitleOutcome, AgentTitleServiceError> {
        if let Some(title) = state.accepted() {
            return self.publish_custodied_title(&claim, title).await;
        }
        // An unresolved effect is never redispatched. Its store grants only a
        // fallback-only claim, which publishes the prompt-derived title below.
        if state.unknown() && claim.mode() != AgentTitleClaimMode::FallbackOnly {
            return Err(AgentTitleServiceError::EffectUnknown);
        }
        let transcript = self
            .store
            .agent_title_transcript(claim.agent_id())
            .await?
            .ok_or(AgentTitleServiceError::InconsistentEvidence)?;
        let (context, generator) = match (context, generator, claim.mode()) {
            (Some(context), Some(generator), AgentTitleClaimMode::Generate) => (context, generator),
            _ => {
                let fallback = AgentTitle::fallback(Some(transcript.owner_message()));
                let claim = if claim.mode() == AgentTitleClaimMode::FallbackOnly {
                    claim.settle_fallback()
                } else {
                    claim
                };
                return Ok(
                    if self
                        .store
                        .complete_agent_title(
                            &claim,
                            &fallback,
                            AgentTitleSource::Fallback,
                            now_rfc3339(),
                        )
                        .await?
                    {
                        AgentTitleOutcome::Fallback
                    } else {
                        AgentTitleOutcome::Skipped
                    },
                );
            }
        };
        let prepared = timeout_at(
            deadline,
            generator.prepare(AgentTitleGenerationRequest::new(context, &transcript)),
        )
        .await
        .map_err(|_| AgentTitleServiceError::GenerationUnavailable)?
        .map_err(|error| {
            error
                .budget_error()
                .map(AgentTitleServiceError::Budget)
                .unwrap_or(AgentTitleServiceError::GenerationUnavailable)
        })?;
        let intent = AgentTitleEffectIntent::new(&claim, &transcript, prepared.as_ref())?;
        // From this point any error/cancellation may leave a committed intent.
        // Only this stack owns the original never-consumed capability.
        if !timeout_at(
            deadline,
            self.store.begin_agent_title_effect(&claim, &intent),
        )
        .await
        .map_err(|_| AgentTitleServiceError::EffectUnknown)??
        {
            return Err(AgentTitleServiceError::EffectUnknown);
        }
        if !timeout_at(
            deadline,
            self.store.validate_agent_title_effect(&claim, &intent),
        )
        .await
        .map_err(|_| AgentTitleServiceError::EffectUnknown)??
        {
            return Err(AgentTitleServiceError::EffectUnknown);
        }
        let response = timeout_at(deadline, prepared.dispatch())
            .await
            .map_err(|_| AgentTitleServiceError::EffectUnknown)?
            .map_err(|_| AgentTitleServiceError::EffectUnknown)?;
        let title =
            AgentTitle::sanitize(response.as_str()).ok_or(AgentTitleServiceError::EffectUnknown)?;
        if !self
            .store
            .accept_agent_title_effect(&claim, &intent, &title)
            .await?
        {
            return Err(AgentTitleServiceError::EffectUnknown);
        }
        self.publish_custodied_title(&claim, &title).await
    }

    async fn publish_custodied_title(
        &self,
        claim: &AgentTitleClaim,
        title: &AgentTitle,
    ) -> Result<AgentTitleOutcome, AgentTitleServiceError> {
        if self
            .store
            .complete_agent_title(claim, title, AgentTitleSource::Generated, now_rfc3339())
            .await?
        {
            Ok(AgentTitleOutcome::Generated)
        } else {
            Ok(AgentTitleOutcome::Skipped)
        }
    }
}
