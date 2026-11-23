use super::*;

impl TitleMutation {
    pub(super) fn settled(&self, state: &TitleState) -> Option<TitleOutcome> {
        if state
            .stored
            .as_ref()
            .is_some_and(|title| title.source != AgentTitleSource::Fallback)
            || matches!(self, Self::User { .. }) && state.stored.is_some()
            || matches!(self, Self::Automatic { claim, .. } if state.completed_by(claim))
        {
            return Some(match self {
                Self::Automatic { claim, .. } => TitleOutcome::Automatic(state.completed_by(claim)),
                Self::User { .. } => TitleOutcome::User(SetAgentTitleOutcome::AlreadySet),
            });
        }
        if let Self::Automatic {
            claim,
            source,
            title,
            ..
        } = self
        {
            // Only a fallback-only claim, taken after the original capability
            // expired, may publish over an unresolved provider effect (#2266).
            if !state.effect.permits_title(title, *source)
                || state.effect.unknown() && claim.mode() != AgentTitleClaimMode::FallbackOnly
            {
                return Some(TitleOutcome::Automatic(false));
            }
            if !state
                .lease
                .as_ref()
                .is_some_and(|lease| lease.matches_claim(claim))
                || claim.mode() == AgentTitleClaimMode::FallbackOnly
                    && *source != AgentTitleSource::Fallback
            {
                return Some(TitleOutcome::Automatic(false));
            }
        }
        None
    }
    pub(super) fn id(&self) -> &AgentId {
        match self {
            Self::Automatic { claim, .. } => claim.agent_id(),
            Self::User { id, .. } => id,
        }
    }

    pub(super) fn title(&self) -> &AgentTitle {
        match self {
            Self::Automatic { title, .. } | Self::User { title, .. } => title,
        }
    }

    pub(super) fn source(&self) -> AgentTitleSource {
        match self {
            Self::Automatic { source, .. } => *source,
            Self::User { .. } => AgentTitleSource::User,
        }
    }

    pub(super) fn set_at(&self) -> &str {
        match self {
            Self::Automatic { set_at, .. } | Self::User { set_at, .. } => set_at,
        }
    }

    pub(super) fn completion_fields(&self) -> Document {
        match self {
            Self::Automatic {
                claim,
                title,
                source,
                set_at,
            } => automatic_completion_fields(claim, title, *source, set_at),
            Self::User {
                title,
                set_at,
                operation_token,
                ..
            } => user_completion_fields(title, set_at, operation_token),
        }
    }

    pub(super) fn proof(&self) -> CompletionProof {
        match self {
            Self::Automatic { claim, .. } => CompletionProof::Automatic(claim.clone()),
            Self::User {
                operation_token, ..
            } => CompletionProof::User(operation_token.clone()),
        }
    }

    pub(super) fn success_outcome(&self) -> TitleOutcome {
        match self {
            Self::Automatic { .. } => TitleOutcome::Automatic(true),
            Self::User { .. } => TitleOutcome::User(SetAgentTitleOutcome::Set),
        }
    }
}
