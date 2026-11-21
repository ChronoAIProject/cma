//! Partition-rule validation for channel definitions.

use super::{PartitionKey, PartitionRule, Source};
use crate::{activity::SourceKind, ValidationError};

const CUSTOM_KEYS: &str = "custom sources require fixed, event_subject, or per_event";

/// Keys a custom event can be partitioned by.
fn is_custom_key(key: PartitionKey) -> bool {
    matches!(
        key,
        PartitionKey::Fixed | PartitionKey::EventSubject | PartitionKey::PerEvent
    )
}

/// Keys that read a chat conversation.
fn is_chat_key(key: PartitionKey) -> bool {
    matches!(
        key,
        PartitionKey::ConversationAndSender | PartitionKey::Conversation | PartitionKey::Thread
    )
}

impl PartitionRule {
    /// The key that applies to an activity from `source`.
    pub fn key_for(&self, source: &SourceKind) -> PartitionKey {
        match source {
            SourceKind::Custom | SourceKind::Github { .. } => self.custom_key.unwrap_or(self.key),
            _ => self.key,
        }
    }

    /// Today's partition rules, kept verbatim as the legacy predicate.
    pub(crate) fn validate_legacy(&self, sources: &[Source]) -> Result<(), ValidationError> {
        if self.version != 1 {
            return Err(ValidationError::new(
                "/partition/version",
                "unsupported version",
            ));
        }
        let has_custom = sources
            .iter()
            .any(|source| matches!(source, Source::Custom { .. }));
        let has_relay = sources.iter().any(|source| {
            matches!(
                source,
                Source::NyxidRelay { .. }
                    | Source::Whatsapp { .. }
                    | Source::Telegram { .. }
                    | Source::Discord { .. }
            )
        });
        if has_custom
            && !matches!(
                self.key,
                PartitionKey::Fixed | PartitionKey::EventSubject | PartitionKey::PerEvent
            )
        {
            return Err(ValidationError::new(
                "/partition/key",
                "custom sources require fixed, event_subject, or per_event",
            ));
        }
        if matches!(
            self.key,
            PartitionKey::EventSubject | PartitionKey::PerEvent
        ) && !has_custom
        {
            return Err(ValidationError::new(
                "/partition/key",
                "requires a custom source",
            ));
        }
        if matches!(self.key, PartitionKey::Thread) && !has_relay {
            return Err(ValidationError::new(
                "/partition/key",
                "requires a chat source",
            ));
        }
        Ok(())
    }

    /// Extended rules, applied only when the legacy rules fail or
    /// `custom_key` is present. Every present source needs a key it can be
    /// partitioned by, but a key no longer needs a present source of its
    /// kind: `thread` without a chat source, and `custom_key` without a
    /// custom source, stay inert until such a source returns.
    pub(crate) fn validate_extended(&self, sources: &[Source]) -> Result<(), ValidationError> {
        if self.version != 1 {
            return Err(ValidationError::new(
                "/partition/version",
                "unsupported version",
            ));
        }
        if let Some(custom_key) = self.custom_key {
            if !is_chat_key(self.key) {
                return Err(ValidationError::new(
                    "/partition/custom_key",
                    "requires a chat partition key",
                ));
            }
            if !is_custom_key(custom_key) {
                return Err(ValidationError::new(
                    "/partition/custom_key",
                    "must be fixed, event_subject, or per_event",
                ));
            }
        }
        for source in sources {
            match source {
                Source::Whatsapp { .. } if self.key == PartitionKey::Thread => {
                    return Err(ValidationError::new(
                        "/partition/key",
                        "native WhatsApp private chats do not have threads",
                    ));
                }
                Source::Custom { .. } | Source::Github { .. }
                    if !is_custom_key(self.key_for(&SourceKind::Custom)) =>
                {
                    return Err(ValidationError::new("/partition/key", CUSTOM_KEYS));
                }
                Source::NativeLark(_)
                | Source::Whatsapp { .. }
                | Source::Telegram { .. }
                | Source::Discord { .. }
                | Source::NyxidRelay { .. }
                    if self.key == PartitionKey::PerEvent =>
                {
                    return Err(ValidationError::new(
                        "/partition/key",
                        "chat sources cannot use per_event",
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }
}
