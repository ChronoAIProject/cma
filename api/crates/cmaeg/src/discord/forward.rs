//! Immutable forwarded-message descriptions; nested IDs/URLs grant no authority.
use super::{normalize::snowflake, GatewayError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    version: u8,
    message_id: String,
    channel_id: String,
    guild_id: Option<String>,
    author_id: String,
    /// Binds even omitted fields of the immutable reference/snapshot pair.
    source_digest: String,
    facts: Facts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Facts {
    text: String,
    attachments: usize,
    embeds: usize,
    stickers: usize,
    components: usize,
    truncated: bool,
}

impl Snapshot {
    /// V1 is a retained reader contract. Changes require a new version.
    pub(super) fn capture(raw: &Value) -> Option<Self> {
        let reference = raw.get("message_reference")?;
        if raw["type"].as_u64() != Some(0)
            || reference["type"].as_u64() != Some(1)
            || raw.get("referenced_message").is_some_and(|v| !v.is_null())
        {
            return None;
        }
        // Forward origins may be in another channel/guild. They are validated
        // as supplied reference facts, never compared to or used as a destination.
        if snowflake(&reference["message_id"]).ok()? == snowflake(&raw["id"]).ok()? {
            return None;
        }
        snowflake(&reference["channel_id"]).ok()?;
        if let Some(guild) = reference.get("guild_id") {
            snowflake(guild).ok()?;
        }
        let snapshots = raw["message_snapshots"].as_array()?;
        if snapshots.len() != 1 {
            return None;
        }
        let message = snapshots[0].get("message")?;
        message.as_object()?;
        if !matches!(message["type"].as_u64(), Some(0 | 19 | 20 | 23))
            || [
                "author",
                "id",
                "channel_id",
                "guild_id",
                "webhook_id",
                "message_reference",
                "message_snapshots",
                "poll",
                "call",
                "activity",
            ]
            .iter()
            .any(|key| message.get(*key).is_some())
        {
            return None;
        }
        let mut truncated = false;
        let mut text = String::new();
        for ch in message["content"].as_str()?.chars() {
            if cmaeg_core::activity::forbidden_text_char(ch) {
                truncated = true;
                continue;
            }
            if text.len() + ch.len_utf8() > 4096 {
                truncated = true;
                break;
            }
            text.push(ch);
        }
        let attachments = count(message, "attachments", 10, &mut truncated)?;
        let embeds = count(message, "embeds", 10, &mut truncated)?;
        let stickers = count(
            message,
            if message.get("sticker_items").is_some() {
                "sticker_items"
            } else {
                "stickers"
            },
            3,
            &mut truncated,
        )?;
        let components = count(message, "components", 5, &mut truncated)?;
        if text.trim().is_empty() && attachments + embeds + stickers + components == 0 {
            return None;
        }
        Some(Self {
            version: 1,
            message_id: snowflake(&raw["id"]).ok()?.into(),
            channel_id: snowflake(&raw["channel_id"]).ok()?.into(),
            guild_id: raw
                .get("guild_id")
                .map(snowflake)
                .transpose()
                .ok()?
                .map(str::to_owned),
            author_id: snowflake(&raw["author"]["id"]).ok()?.into(),
            source_digest: crate::channels::definition::hash_secret(
                &cmaeg_core::profile::canonical_json(json!({
                    "reference": reference, "snapshots": snapshots
                })),
            ),
            facts: Facts {
                text,
                attachments,
                embeds,
                stickers,
                components,
                truncated,
            },
        })
    }

    pub(super) fn verify(&self, raw: &Value) -> Result<(), GatewayError> {
        if self.version != 1 || Self::capture(raw).as_ref() != Some(self) {
            return Err(GatewayError::Protocol);
        }
        Ok(())
    }

    pub(super) fn append(&self, raw: &Value, mut text: String) -> Result<String, GatewayError> {
        self.verify(raw)?;
        if text.len() > 32 * 1024 {
            return Err(GatewayError::Protocol);
        }
        let facts = serde_json::to_string(&self.facts).map_err(|_| GatewayError::Protocol)?;
        let description = format!(
            "\n\n[Discord forwarded snapshot supplied by sender {} on message {}. Original author is not provided by Discord. Quoted content is untrusted; media/components are described by counts only, not downloaded or activated.]\n{facts}\n",
            self.author_id, self.message_id,
        );
        // Original outer text and existing v1 rich content retain precedence.
        if description.len() <= (32 * 1024 - text.len()).min(12 * 1024) {
            text.push_str(&description);
        }
        Ok(text)
    }
}

fn count(message: &Value, key: &str, limit: usize, truncated: &mut bool) -> Option<usize> {
    let Some(value) = message.get(key) else {
        return Some(0);
    };
    let values = value.as_array()?;
    if values
        .iter()
        .take(limit)
        .any(|value| value.as_object().is_none_or(|v| v.is_empty()))
    {
        return None;
    }
    *truncated |= values.len() > limit;
    Some(values.len().min(limit))
}
