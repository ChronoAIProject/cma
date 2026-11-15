use super::*;

pub(super) struct FencedPrepared {
    pub inner: Box<dyn PreparedAgentTitle>,
    pub worker: Arc<AgentTitleWorker>,
    pub authority: TitleAuthority,
    pub request: AgentTitleGenerationRequest,
}
#[async_trait::async_trait]
impl PreparedAgentTitle for FencedPrepared {
    fn request_digest(&self) -> &str {
        self.inner.request_digest()
    }
    fn strategy(&self) -> &str {
        self.inner.strategy()
    }
    async fn dispatch(
        self: Box<Self>,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        if !self
            .worker
            .is_current(&self.authority, self.request.bearer())
            .await
        {
            return Err(AgentTitleGenerationError::unavailable());
        }
        let result = self.inner.dispatch().await?;
        if !self
            .worker
            .is_current(&self.authority, self.request.bearer())
            .await
        {
            return Err(AgentTitleGenerationError::unavailable());
        }
        Ok(result)
    }
}
