use layer2_core::profiles::SkillRef;
use layer3_adapter::AgentSkill;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AgentSkillResolutionError {
    #[error("skill source is temporarily unavailable: {0}")]
    Unavailable(String),
    #[error("skill reference is unavailable: {0}")]
    NotFound(String),
    #[error("skill source returned invalid material: {0}")]
    Invalid(String),
}

/// Resolves profile references to immutable archives before home materialization.
#[async_trait::async_trait]
pub(crate) trait AgentSkillResolver: Send + Sync {
    async fn resolve(
        &self,
        references: &[SkillRef],
    ) -> Result<Vec<AgentSkill>, AgentSkillResolutionError>;
}

#[derive(Default)]
pub(crate) struct EmptyAgentSkillResolver;

#[async_trait::async_trait]
impl AgentSkillResolver for EmptyAgentSkillResolver {
    async fn resolve(
        &self,
        references: &[SkillRef],
    ) -> Result<Vec<AgentSkill>, AgentSkillResolutionError> {
        if references.is_empty() {
            Ok(Vec::new())
        } else {
            Err(AgentSkillResolutionError::Unavailable(
                "no agent skill source is composed".into(),
            ))
        }
    }
}
