use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{ids::ConsumerId, seal::EventRefPayload};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::time::Instant;
pub(super) type EventKey = [u8; 32];
pub(super) fn normalized_consumer(consumer: Option<&ConsumerId>) -> Option<&ConsumerId> {
    consumer.filter(|id| !id.is_default())
}
pub(super) fn key(channel: &str, event: &str, consumer: Option<&ConsumerId>) -> EventKey {
    let mut digest = Sha256::new();
    // Keep the legacy default bytes and worker assignment exactly unchanged.
    // A named consumer owns its own terminal, acknowledgement and suppression state.
    if let Some(consumer) = normalized_consumer(consumer) {
        digest.update(b"cmaeg.delivery.consumer.v1\0");
        digest.update(consumer.as_str().as_bytes());
        digest.update([0]);
    }
    digest.update(channel.as_bytes());
    digest.update([0]);
    digest.update(event.as_bytes());
    digest.finalize().into()
}
pub(super) struct EventState {
    pub thread_reply: bool,
    pub trace: tracing::Span,
    pub channel: Arc<ResolvedChannel>,
    pub event: EventRefPayload,
    pub first_send_done: bool,
    pub ack_sent: bool,
    pub ack_due: Option<Instant>,
    pub last_output_at: Instant,
    pub last_progress_at: Instant,
    pub heartbeats: u8,
    pub tool_replied: bool,
    pub answer_attempted: bool,
    pub accepted_deadline: Option<Instant>,
    pub terminal_at: Option<Instant>,
    pub terminal: bool,
}
impl EventState {
    pub fn new(channel: Arc<ResolvedChannel>, event: EventRefPayload) -> Self {
        let now = Instant::now();
        Self {
            thread_reply: false,
            trace: tracing::Span::current(),
            channel,
            event,
            first_send_done: false,
            ack_sent: false,
            ack_due: None,
            last_output_at: now,
            last_progress_at: now,
            heartbeats: 0,
            tool_replied: false,
            answer_attempted: false,
            accepted_deadline: None,
            terminal_at: Some(now),
            terminal: false,
        }
    }
}
pub(super) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_delivery_identity_keeps_its_legacy_hash_and_named_identities_are_separate() {
        let legacy = key("cmaeg1.ch.recording-relay", "msg-1", None);
        let hex: String = legacy.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            hex,
            "cfe428bc6f7d7a2fbf503b53253d66ce8bf010232ceb4000607cd5ed86832368"
        );
        assert_eq!(
            legacy,
            key(
                "cmaeg1.ch.recording-relay",
                "msg-1",
                Some(&ConsumerId::default_consumer())
            )
        );
        let a = ConsumerId::parse("csm_00000000000a").unwrap();
        let b = ConsumerId::parse("csm_00000000000b").unwrap();
        let named = key("cmaeg1.ch.recording-relay", "msg-1", Some(&a));
        assert_ne!(named, legacy);
        assert_ne!(named, key("cmaeg1.ch.recording-relay", "msg-1", Some(&b)));
    }
}
