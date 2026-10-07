//! The authority to read one event's attachments: its current channel, its
//! opened event_ref and the activity that event_ref sealed.
use super::cache::CacheKey;
use crate::{channels::resolver::ResolvedChannel, telemetry, turns::render_media};
use cmaeg_core::{
    activity::{Activity, Attachment},
    seal::EventRefPayload,
};
use std::{fmt, sync::Arc};

/// A verified pairing of an activity with the event_ref that authorizes
/// fetching its attachments.
///
/// The event_ref carries the reply address (for a relayed message, NyxID's
/// message ID and reply token) that sources need, and the activity digest
/// that proves the attachment locators are the ones the source delivered.
#[derive(Clone)]
pub struct MediaTarget {
    pub channel: Arc<ResolvedChannel>,
    pub event: EventRefPayload,
    pub activity: Arc<Activity>,
}

/// Why an activity and an event_ref do not authorize media reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetError {
    /// The event_ref belongs to another channel.
    ForeignChannel,
    /// The activity is not the one the event_ref sealed.
    DigestMismatch,
    /// The event_ref does not open, has expired, or seals no activity digest.
    Unverifiable,
}

impl TargetError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ForeignChannel => "foreign_channel",
            Self::DigestMismatch => "digest_mismatch",
            Self::Unverifiable => "unverifiable",
        }
    }
}

impl fmt::Display for TargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::error::Error for TargetError {}

impl MediaTarget {
    /// Pair `activity` with `event` on `channel`, refusing any mismatch.
    pub fn new(
        channel: Arc<ResolvedChannel>,
        event: EventRefPayload,
        activity: Arc<Activity>,
    ) -> Result<Self, TargetError> {
        if event.channel_id != channel.id {
            return Err(TargetError::ForeignChannel);
        }
        if event.activity_sha256.is_none() {
            return Err(TargetError::Unverifiable);
        }
        // The digest covers every locator, so a matching digest proves the
        // attachments are exactly what the source delivered.
        if !event.verifies_activity(&activity) {
            return Err(TargetError::DigestMismatch);
        }
        Ok(Self {
            channel,
            event,
            activity,
        })
    }

    /// Attachment `index` of a message or event activity.
    pub fn attachment(&self, index: usize) -> Option<&Attachment> {
        render_media::attachments(&self.activity).get(index)
    }

    /// The derived-cache key of `artifact` for attachment `index`.
    pub fn cache_key(&self, index: u8, artifact: &'static str) -> CacheKey {
        CacheKey::new(
            &self.channel.id,
            &self.event.event_id,
            self.event.activity_sha256.as_deref().unwrap_or_default(),
            index,
            artifact,
        )
    }
}

/// The reply address carries a reply token, and identifiers are not for
/// logs, so only the channel hash and the event ID are shown.
impl fmt::Debug for MediaTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaTarget")
            .field(
                "channel",
                &telemetry::channel_hash(self.channel.id.as_str()),
            )
            .field("event", &self.event.event_id.as_str())
            .finish_non_exhaustive()
    }
}
