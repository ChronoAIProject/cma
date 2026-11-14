use super::*;

impl InMemoryAgentTitleStore {
    pub(super) fn validate_effect(
        &self,
        claim: &AgentTitleClaim,
        intent: &AgentTitleEffectIntent,
    ) -> Result<bool, AgentTitleStoreError> {
        intent.validate()?;
        let state = self.lock();
        let agent = state
            .agents
            .get(claim.agent_id())
            .ok_or(AgentTitleStoreError::NotFound)?;
        Ok(live(agent, claim, intent)
            && matches!(&agent.effect,
            AgentTitleEffectState::Retained(effect) if &effect.intent == intent && effect.result.is_none()))
    }
    pub(super) fn mutate_effect(
        &self,
        claim: &AgentTitleClaim,
        intent: &AgentTitleEffectIntent,
        title: Option<&AgentTitle>,
    ) -> Result<bool, AgentTitleStoreError> {
        intent.validate()?;
        let mut state = self.lock();
        let agent = state
            .agents
            .get_mut(claim.agent_id())
            .ok_or(AgentTitleStoreError::NotFound)?;
        if let AgentTitleEffectState::Retained(effect) = &agent.effect {
            if &effect.intent != intent {
                return Ok(false);
            }
            if let Some(result) = &effect.result {
                return Ok(title == Some(result));
            }
        }
        if !live(agent, claim, intent) {
            return Ok(false);
        }
        match (&agent.effect, title) {
            (AgentTitleEffectState::Ready, None) => {
                agent.effect = AgentTitleEffectState::Retained(Box::new(AgentTitleEffect {
                    version: 1,
                    intent: intent.clone(),
                    result: None,
                }));
            }
            (AgentTitleEffectState::Retained(_), _) => {
                agent.effect = AgentTitleEffectState::Retained(Box::new(AgentTitleEffect {
                    version: 1,
                    intent: intent.clone(),
                    result: title.cloned(),
                }));
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
fn live(agent: &MemoryAgent, claim: &AgentTitleClaim, intent: &AgentTitleEffectIntent) -> bool {
    intent.matches_claim(claim)
        && agent.work.lease.as_ref() == Some(claim)
        && claim.expires_at() > SystemTime::now()
        && agent
            .title
            .as_ref()
            .is_none_or(|title| title.source == AgentTitleSource::Fallback)
}
