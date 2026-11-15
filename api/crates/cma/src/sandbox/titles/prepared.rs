use super::*;

pub(super) struct Prepared {
    pub generator: HttpAgentTitleGenerator,
    pub request: AgentTitleGenerationRequest,
    pub frozen: reqwest::Request,
    pub permit: Option<crate::sandbox::model_request_budget::RequestPermit>,
    pub deadline: tokio::time::Instant,
    pub request_digest: String,
}

#[async_trait]
impl PreparedAgentTitle for Prepared {
    fn request_digest(&self) -> &str {
        &self.request_digest
    }
    fn strategy(&self) -> &str {
        "automatic-title-v1"
    }
    async fn dispatch(
        self: Box<Self>,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(AgentTitleGenerationError::unavailable());
        }
        // The request and permit are moved, never cloned or reconstructed after
        // a retained intent. Cancellation conservatively leaves the hold intact.
        tokio::time::timeout_at(self.deadline, async {
            let response = self
                .generator
                .client
                .execute(self.frozen)
                .await
                .map_err(|_| AgentTitleGenerationError::unavailable())?;
            self.generator
                .receive(&self.request, response, self.permit)
                .await
        })
        .await
        .map_err(|_| AgentTitleGenerationError::unavailable())?
    }
}
