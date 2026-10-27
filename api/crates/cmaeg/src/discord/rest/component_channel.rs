//! Authenticated component destination preflight, before any durable card exists.
use super::{valid_id, DiscordRest, RestError};
use crate::discord::component_thread::ThreadIdentity;
use cmaeg_core::channel::Secret;
use std::time::Duration;

impl DiscordRest {
    /// Missing thread metadata in an event is not proof of an ordinary channel.
    /// Resolve the exact destination before creating an interactive card; failed
    /// or unknown evidence must never produce controls whose clicks we refuse.
    pub(crate) async fn verify_component_channel(
        &self,
        token: &Secret,
        channel: &str,
        conversation: &cmaeg_core::activity::ActivityConversation,
    ) -> Result<Option<ThreadIdentity>, RestError> {
        if !valid_id(channel) {
            return Err(RestError::Rejected);
        }
        self.ready(token)?;
        let body = tokio::time::timeout(Duration::from_secs(2), async {
            let response = self
                .client
                .get(self.url(&format!("channels/{channel}"))?)
                .header("Authorization", format!("Bot {}", token.as_str()))
                .send()
                .await
                .map_err(|_| RestError::Unavailable)?;
            self.decode(response, token, false).await
        })
        .await
        .map_err(|_| RestError::Unavailable)??;
        if body["id"].as_str() != Some(channel) || conversation.id != channel {
            return Err(RestError::Rejected);
        }
        if conversation.thread_id.is_some() {
            let thread = ThreadIdentity::from_channel(&body).ok_or(RestError::Rejected)?;
            return thread
                .matches(conversation)
                .then_some(Some(thread))
                .ok_or(RestError::Rejected);
        }
        let ordinary = match conversation.kind {
            cmaeg_core::activity::ConversationKind::Private => {
                body["type"].as_u64() == Some(1) && body["guild_id"].is_null()
            }
            cmaeg_core::activity::ConversationKind::Group => {
                matches!(body["type"].as_u64(), Some(0 | 2 | 5 | 13))
                    && body["guild_id"].as_str().is_some_and(valid_id)
            }
            _ => false,
        };
        if !ordinary {
            return Err(RestError::Rejected);
        }
        Ok(None)
    }
}
