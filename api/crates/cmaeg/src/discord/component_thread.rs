//! Authenticated existing-thread identity retained with component authority.
use super::{DiscordApi, GatewayError};
use crate::{
    channels::resolver::ResolvedChannel,
    interactions::{Error, InteractionStore},
};
use cmaeg_core::activity::{Activity, ActivityConversation, ConversationKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadIdentity {
    channel_id: String,
    channel_type: u8,
    guild_id: String,
    parent_id: String,
}
impl ThreadIdentity {
    pub(crate) fn from_channel(value: &Value) -> Option<Self> {
        let result = Self {
            channel_id: value["id"].as_str()?.into(),
            channel_type: u8::try_from(value["type"].as_u64()?).ok()?,
            guild_id: value["guild_id"].as_str()?.into(),
            parent_id: value["parent_id"].as_str()?.into(),
        };
        result.valid().then_some(result)
    }
    fn valid(&self) -> bool {
        matches!(self.channel_type, 10..=12)
            && [&self.channel_id, &self.guild_id, &self.parent_id]
                .into_iter()
                .all(|id| cmaeg_core::channel::valid_discord_id(id))
            && self.parent_id != self.channel_id
    }
    pub(crate) fn matches(&self, conversation: &ActivityConversation) -> bool {
        self.valid()
            && conversation.kind == ConversationKind::Group
            && conversation.id == self.channel_id
            && conversation.thread_id.as_ref() == Some(&self.channel_id)
    }
    pub(crate) fn optional_matches(
        thread: Option<&Self>,
        conversation: &ActivityConversation,
    ) -> bool {
        match thread {
            Some(thread) => thread.matches(conversation),
            None => conversation.thread_id.is_none(),
        }
    }
    fn matches_callback(&self, raw: &Value) -> bool {
        raw["channel_id"].as_str() == Some(&self.channel_id)
            && raw["guild_id"].as_str() == Some(&self.guild_id)
            && raw
                .pointer("/channel/type")
                .is_none_or(|kind| kind.as_u64() == Some(u64::from(self.channel_type)))
            && raw
                .pointer("/channel/guild_id")
                .is_none_or(|id| id.as_str() == Some(&self.guild_id))
            && raw
                .pointer("/channel/parent_id")
                .is_none_or(|id| id.as_str() == Some(&self.parent_id))
    }
}

impl DiscordApi {
    /// Existing consumed cards may finish a lost inbox write after the original
    /// deadline. Final consumption still requires the exact prior click. Fresh
    /// clicks authenticate current metadata within the original ACK budget.
    pub(super) async fn component_thread(
        &self,
        channel: &ResolvedChannel,
        store: &InteractionStore,
        activity: &mut Activity,
        raw: &Value,
        deadline: tokio::time::Instant,
    ) -> Result<Option<ThreadIdentity>, Error> {
        let custom = raw["data"]["custom_id"].as_str().ok_or(Error::Denied)?;
        let (thread, consumed) = store.discord_thread(custom).await?;
        let Some(thread) = thread else {
            return if matches!(
                raw.pointer("/channel/type").and_then(Value::as_u64),
                Some(10..=12)
            ) {
                Err(Error::Denied)
            } else {
                Ok(None)
            };
        };
        if !thread.matches_callback(raw) {
            return Err(Error::Denied);
        }
        if !consumed {
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Denied);
            }
            let cmaeg_core::activity::SourceKind::Discord { bot_id } = &activity.source else {
                return Err(Error::Denied);
            };
            let token = super::rest::token(channel, bot_id).ok_or(Error::Denied)?;
            let current: Value = tokio::time::timeout_at(
                deadline,
                self.get(&format!("channels/{}", thread.channel_id), token),
            )
            .await
            .map_err(|_| Error::Denied)?
            .map_err(|error| match error {
                GatewayError::Unavailable | GatewayError::RateLimited => Error::Unavailable,
                _ => Error::Denied,
            })?;
            if ThreadIdentity::from_channel(&current).as_ref() != Some(&thread) {
                return Err(Error::Denied);
            }
        }
        activity.conversation.thread_id = Some(thread.channel_id.clone());
        if !thread.matches(&activity.conversation) {
            return Err(Error::Denied);
        }
        Ok(Some(thread))
    }
}
