use serde::{Deserialize, Serialize};

/// Retained reaction wire shape for a confirmed CMA message reference.
/// Lark/Feishu use `Emoji` with a platform emoji_type such as `THUMBSUP`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TelegramReaction {
    /// A supported Unicode emoji, or a Lark/Feishu platform emoji_type.
    Emoji {
        /// The reaction emoji.
        emoji: String,
    },
    /// A custom emoji already allowed by Telegram for the message.
    CustomEmoji {
        /// Telegram's numeric custom emoji identifier.
        custom_emoji_id: String,
    },
}
/// Set one bot reaction, or clear the bot's reaction with an empty list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageReactionRequest {
    /// Zero or one reaction; paid reactions are unsupported.
    pub reaction: Vec<TelegramReaction>,
    /// Ask Telegram to display its large reaction animation.
    #[serde(default)]
    pub is_big: bool,
}
/// Persisted outcome of one explicit, idempotent message effect.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageEffectReceipt {
    /// Confirmed, rejected, unknown, rate-limited, or unsupported outcome.
    pub delivery: super::DeliveryOutcome,
}
/// Exact-message transport support; platform permissions and limits still apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageEffectsCapabilities {
    /// Whether the message reference grants native deletion authority.
    pub delete: bool,
    /// Whether the message reference grants native bot-reaction authority.
    pub reaction: bool,
    /// The authorized scope, currently `confirmed_native_bot_message`.
    pub scope: String,
    /// Maximum supported bot reactions: one when supported, otherwise zero.
    pub max_reactions: u8,
    /// Unix-seconds expiry of the effect authority, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}
/// Empty body required by the exact-message deletion operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EmptyEffectRequest {}
