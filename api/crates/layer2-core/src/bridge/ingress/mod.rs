//! Signed NyxID callback acceptance. A verified callback grants only permission
//! to persist an inbox entry; deferred work requires separate current authority.
mod capabilities;
mod control;
mod jwt;
mod model;
mod payload;
mod redelivery;
mod service;
mod text_attachment;
pub use text_attachment::{snowflake as discord_snowflake, valid_text as valid_attachment_text};
pub use text_attachment::{
    CapturedAttachmentText, DiscordTextAttachment, VerifiedTextAttachment,
    MAX_CHANNEL_TEXT_ATTACHMENT_BYTES,
};

pub use capabilities::{AuthenticatedCallbackPayload, CallbackCapabilityExtractor};
pub use control::{
    CallbackAcceptance, ChannelControlAcceptedReceipt, ChannelControlReceipt, ChannelControlRefusal,
};
pub use jwt::{CallbackKeySet, CallbackTokenPolicy};
pub use model::*;
pub use redelivery::{VerifiedPlatformRedelivery, VerifiedRedeliveryParent};
pub use service::{CallbackHeaders, CallbackVerifier};

#[cfg(test)]
mod tests;
