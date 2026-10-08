//! Which sends are routed. A routed send may carry a label; an unrouted one
//! is exactly today's.
use cmaeg_core::{
    channel::ChannelDefinition,
    seal::{EventRefPayload, MessageRefPayload},
};

/// What kind of send is being classified (Amendment R6 §R6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendKind {
    /// An automatic output of a turn: ack, heartbeat, answer, failure text.
    /// `routed_turn` is `turn.activity.routing.is_some()`, fixed for the turn's life.
    Output { routed_turn: bool },
    /// A tool command, classified once by its handler from the snapshot it resolved;
    /// the worker reuses that snapshot and the same process-wide flag.
    Tool,
}

/// Evaluated from the channel snapshot the command carries, so it is
/// deterministic after the event's group expires (24-hour reply targets).
/// Always false for an unrouted turn's outputs, so a turn admitted unrouted
/// keeps today's behaviour if its channel becomes routed mid-turn. With the
/// flag off, only a reference bound to a consumer or a hand-off is routed:
/// `default` references carry no marker (I3).
pub(crate) fn routing_send(
    def: &ChannelDefinition,
    payload: &EventRefPayload,
    multi_consumer_enabled: bool,
    kind: SendKind,
) -> bool {
    let routed = def.routing.is_some()
        && (multi_consumer_enabled || payload.consumer.is_some() || payload.lineage.is_some());
    routed
        && match kind {
            SendKind::Output { routed_turn } => routed_turn,
            SendKind::Tool => true,
        }
}

/// The same rule for `updateMessage`, whose `MessageRefPayload` carries
/// `consumer` but no lineage.
pub(crate) fn routing_send_for_message(
    def: &ChannelDefinition,
    m: &MessageRefPayload,
    multi_consumer_enabled: bool,
) -> bool {
    def.routing.is_some() && (multi_consumer_enabled || m.consumer.is_some())
}

#[cfg(test)]
#[path = "address_tests.rs"]
mod tests;
