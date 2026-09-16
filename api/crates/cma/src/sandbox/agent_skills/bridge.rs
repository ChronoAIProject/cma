//! Public skill preflight uses the current explicit Bridge OAuth lease only.
use super::*;

impl OrnnAgentSkillResolver {
    pub(crate) async fn resolve_public_with_credential(
        &self,
        credential: &str,
        references: &[SkillRef],
    ) -> Result<Vec<ResolvedAgentSkill>, AgentSkillResolutionError> {
        if references.is_empty() {
            return Ok(Vec::new());
        }
        if references.len() > MAX_REFERENCES || credential.trim().is_empty() {
            return Err(invalid(
                0,
                "public skill preflight authority is unavailable",
            ));
        }
        let requests = references
            .iter()
            .enumerate()
            .map(|(index, reference)| self.resolve_one(index, reference, Some(credential), true))
            .collect::<Vec<_>>();
        let material = tokio::time::timeout(
            RESOLUTION_TIMEOUT,
            futures_util::stream::iter(requests)
                .buffered(LOOKUP_CONCURRENCY)
                .try_collect::<Vec<_>>(),
        )
        .await
        .map_err(|_| unavailable("public skill preflight deadline elapsed"))??;
        validate_skill_selection(references, &material)?;
        Ok(material)
    }
}
