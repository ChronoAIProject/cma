use super::*;
use crate::store::{MongoBridgeReplyToolStore, MongoChannelToolStore};
use layer2_core::bridge::outbox::{BridgeReplyToolStore, ChannelReplyToolIntent};

pub(crate) struct StoredChannelTools {
    pub capabilities: Arc<MongoChannelToolStore>,
    pub replies: Arc<MongoBridgeReplyToolStore>,
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
    ) -> Result<Option<String>, OutboxError> {
        self.replies.control_context(session).await
    }
}
