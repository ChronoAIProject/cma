//! Persisted deployment-wide admission for automatic title calls and backfill.
use layer2_core::sandbox::AgentTitleServiceError;

#[async_trait::async_trait]
pub(crate) trait TitleBudget: Send + Sync {
    async fn admit(&self) -> Result<bool, AgentTitleServiceError>;
}
#[async_trait::async_trait]
impl TitleBudget for crate::store::request_limits::MongoRequestLimits {
    async fn admit(&self) -> Result<bool, AgentTitleServiceError> {
        self.check("background:agent-titles", 6)
            .await
            .map(|retry| retry.is_none())
            .map_err(|_| AgentTitleServiceError::GenerationUnavailable)
    }
}
