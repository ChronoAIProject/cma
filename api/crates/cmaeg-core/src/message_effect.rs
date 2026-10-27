//! Bounded message reaction input, shared by REST and MCP.
//! Lark/Feishu use the emoji field for their platform emoji_type names.
use crate::ValidationError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TelegramReaction {
    Emoji { emoji: String },
    CustomEmoji { custom_emoji_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageReactionRequest {
    /// Empty clears the bot's reaction; the gateway supports at most one.
    #[schema(max_items = 1)]
    pub reaction: Vec<TelegramReaction>,
    /// Telegram-only presentation hint; Discord and Lark/Feishu refuse true.
    #[serde(default)]
    pub is_big: bool,
}
impl MessageReactionRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.reaction.len() > 1 {
            return Err(ValidationError::new(
                "/reaction",
                "bots may set at most one reaction",
            ));
        }
        for reaction in &self.reaction {
            let valid = match reaction {
                TelegramReaction::Emoji { emoji } => {
                    !emoji.is_empty() && emoji.len() <= 64 && !emoji.chars().any(char::is_control)
                }
                TelegramReaction::CustomEmoji { custom_emoji_id } => {
                    !custom_emoji_id.is_empty()
                        && custom_emoji_id.len() <= 128
                        && custom_emoji_id.bytes().all(|b| b.is_ascii_digit())
                }
            };
            if !valid {
                return Err(ValidationError::new("/reaction", "invalid reaction"));
            }
        }
        Ok(())
    }
}
