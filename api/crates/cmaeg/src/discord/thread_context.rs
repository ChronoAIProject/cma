//! Exact authenticated channel evidence, retained with the immutable inbox.
use super::{discovery::DiscordApi, normalize::snowflake, GatewayError};
use cmaeg_core::{
    activity::{Activity, ConversationKind},
    channel::Secret,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ChannelEvidence {
    Verified {
        channel_id: String,
        channel_type: u8,
        guild_id: Option<String>,
        parent_id: Option<String>,
    },
    Unavailable,
}

impl DiscordApi {
    pub(super) async fn channel_evidence(
        &self,
        token: &Secret,
        dispatch: &Value,
    ) -> ChannelEvidence {
        self.read_channel_evidence(token, dispatch)
            .await
            .unwrap_or(ChannelEvidence::Unavailable)
    }

    async fn read_channel_evidence(
        &self,
        token: &Secret,
        dispatch: &Value,
    ) -> Result<ChannelEvidence, GatewayError> {
        let channel = snowflake(&dispatch["channel_id"])?;
        // The Gateway MESSAGE_CREATE.thread describes a thread started from
        // the message, not the channel containing it. Never use it here.
        let value: Value = self.get(&format!("channels/{channel}"), token).await?;
        let channel_id = snowflake(&value["id"])?.to_owned();
        let channel_type = value["type"]
            .as_u64()
            .and_then(|value| u8::try_from(value).ok())
            .ok_or(GatewayError::Protocol)?;
        let optional_id = |name| -> Result<Option<String>, GatewayError> {
            match value.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => Ok(Some(snowflake(value)?.to_owned())),
            }
        };
        let evidence = ChannelEvidence::Verified {
            channel_id,
            channel_type,
            guild_id: optional_id("guild_id")?,
            parent_id: optional_id("parent_id")?,
        };
        evidence.thread_id(dispatch)?;
        Ok(evidence)
    }
}

impl ChannelEvidence {
    /// Revalidate persisted evidence against the original dispatch on every
    /// reader path. None means a proven ordinary channel, never missing proof.
    fn thread_id<'a>(&'a self, dispatch: &Value) -> Result<Option<&'a str>, GatewayError> {
        let Self::Verified {
            channel_id,
            channel_type,
            guild_id,
            parent_id,
        } = self
        else {
            return Err(GatewayError::Unavailable);
        };
        let valid = cmaeg_core::channel::valid_discord_id;
        if !valid(channel_id)
            || dispatch["channel_id"].as_str() != Some(channel_id)
            || guild_id.as_deref() != dispatch.get("guild_id").and_then(Value::as_str)
            || guild_id.as_deref().is_some_and(|id| !valid(id))
            || parent_id
                .as_deref()
                .is_some_and(|id| !valid(id) || id == channel_id)
        {
            return Err(GatewayError::Authority);
        }
        match channel_type {
            10..=12 if guild_id.is_some() && parent_id.is_some() => Ok(Some(channel_id)),
            0 | 2 | 5 | 13 if guild_id.is_some() => Ok(None),
            1 | 3 if guild_id.is_none() && parent_id.is_none() => Ok(None),
            _ => Err(GatewayError::Protocol),
        }
    }

    pub(super) fn apply(&self, dispatch: &Value, activity: &mut Activity) -> bool {
        if let Self::Verified { channel_type, .. } = self {
            // Do not grant thread partitioning with admission evaluated against
            // private-message facts when the authenticated channel is a group.
            let expected = if *channel_type == 1 {
                ConversationKind::Private
            } else {
                ConversationKind::Group
            };
            if activity.conversation.kind != expected {
                return false;
            }
        }
        match self.thread_id(dispatch) {
            Ok(thread) => {
                activity.conversation.thread_id = thread.map(str::to_owned);
                true
            }
            Err(_) => false,
        }
    }
}
