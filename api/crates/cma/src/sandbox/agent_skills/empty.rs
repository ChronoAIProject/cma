use super::*;

#[derive(Default)]
pub(crate) struct EmptyAgentSkillResolver;

#[async_trait::async_trait]
impl AgentSkillResolver for EmptyAgentSkillResolver {
    async fn resolve(
        &self,
        _sandbox: &Sandbox,
        references: &[SkillRef],
    ) -> Result<Vec<ResolvedAgentSkill>, AgentSkillResolutionError> {
        if references.is_empty() {
            Ok(Vec::new())
        } else {
            Err(AgentSkillResolutionError::Unavailable(
                "no agent skill source is composed".into(),
            ))
        }
    }

    fn runtime_skills(
        &self,
        material: &[ResolvedAgentSkill],
    ) -> Result<Vec<AgentSkill>, AgentSkillResolutionError> {
        if material.is_empty() {
            Ok(Vec::new())
        } else {
            Err(invalid(0, "no runtime skill source is composed"))
        }
    }
}
