use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::seal::EventRefPayload;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::time::Instant;
pub(super) type EventKey = [u8; 32];
pub(super) fn key(channel: &str, event: &str) -> EventKey {
    let mut digest = Sha256::new();
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
            terminal_at: Some(now),
            terminal: false,
        }
    }
}
pub(super) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
