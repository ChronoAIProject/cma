//! Partition-rule validation for channel definitions.

use super::{PartitionKey, PartitionRule, Source};
use crate::ValidationError;

impl PartitionRule {
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
        let has_relay = sources
            .iter()
            .any(|source| matches!(source, Source::NyxidRelay { .. } | Source::Telegram { .. }));
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
}
