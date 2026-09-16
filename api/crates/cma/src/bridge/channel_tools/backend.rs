use super::*;
use crate::store::{MongoBridgeReplyToolStore, MongoChannelToolStore};
use layer2_core::bridge::outbox::{BridgeReplyToolStore, ChannelReplyToolIntent};

pub(crate) struct StoredChannelTools {
    pub capabilities: Arc<MongoChannelToolStore>,
    pub replies: Arc<MongoBridgeReplyToolStore>,
    pub control_runtime: Option<Arc<super::super::control_runtime::LocalChannelControlRuntime>>,
}

#[async_trait::async_trait]
impl ChannelToolBackend for StoredChannelTools {
    async fn authorize_connection(
        &self,
        credential: &RuntimeCredential,
    ) -> Result<(), OutboxError> {
        self.capabilities.authorize_connection(credential).await
    }
    async fn authenticate(
        &self,
        credential: &RuntimeCredential,
    ) -> Result<ChannelReplySessionAuthorization, OutboxError> {
        self.capabilities.authenticate(credential).await
    }
    async fn context(
        &self,
        session: &ChannelReplySessionAuthorization,
    ) -> Result<Option<String>, OutboxError> {
        self.replies.current_context(session).await
    }
    async fn reply(
        &self,
        session: &ChannelReplySessionAuthorization,
        context: &str,
        request: ChannelReplyToolRequest,
    ) -> Result<ReplyToolReceipt, OutboxError> {
        // Recover the immutable original receipt first, even after its turn has
        // completed or a fresh runtime took over. A retry cannot target a new turn.
        if let Some(receipt) = self.replies.recover(session, &request).await? {
            return Ok(receipt);
        }
        let scope = self.replies.current_turn(session, context).await?;
        self.replies
            .enqueue(ChannelReplyToolIntent::prepare(scope, request)?)
            .await
    }
    async fn control_context(
        &self,
        session: &ChannelReplySessionAuthorization,
    ) -> Result<ChannelControlContext, OutboxError> {
        let Some(observed) = self.replies.control_context_observation(session).await? else {
            return Ok(ChannelControlContext::default());
        };
        let mut result = ChannelControlContext {
            reference: Some(observed.reference),
            operations: Vec::new(),
        };
        if let Some(runtime) = &self.control_runtime {
            if let Some(capabilities) = runtime
                .capabilities(&observed.target, session.deadline())
                .await
            {
                for (name, operation) in [
                    ("cancel", layer2_core::RuntimeCapability::Interrupt),
                    ("steer", layer2_core::RuntimeCapability::Steer),
                ] {
                    if observed.capabilities.supports(operation) && capabilities.supports(operation)
                    {
                        result.operations.push(name);
                    }
                }
            }
        }
        session.require_current()?;
        Ok(result)
    }
}
