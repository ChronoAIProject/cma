//! Which sends are routed, and where a routed send goes. A routed send may
//! carry a label and resolve a destination; an unrouted one is exactly
//! today's: no label, no default destination, the current branch.
use crate::telemetry::names;
use cmaeg_core::{
    channel::{ChannelDefinition, DefaultReply, ReplyChoice},
    message::OutboundMessage,
    seal::{EventRefPayload, MessageRefPayload, ReplyAddress},
};

/// The refusal of a send addressed to the channel's default destination,
/// which no transport delivers to yet.
pub(crate) const DEFAULT_REPLY_UNAVAILABLE: &str = "default_reply_unavailable";
/// The refusal of interactive controls on a hand-off.
pub(crate) const INTERACTIVE_DESTINATION_UNSUPPORTED: &str = "interactive_destination_unsupported";

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

/// Where a send goes once its destination is resolved.
pub(crate) enum EffectiveAddress {
    /// The sealed address of the event's origin.
    Origin(ReplyAddress),
    /// Nothing may be sent; an output for a private destination is never
    /// redirected to a possibly public origin.
    Rejected { reason: &'static str },
}

/// Which delivery branch an address takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddressClass {
    Chat,
    Custom,
}

impl AddressClass {
    /// Telegram and NyxID relay addresses are chats; Sync, result webhooks
    /// and none are custom sinks.
    pub fn of(address: &ReplyAddress) -> Self {
        match address {
            ReplyAddress::Telegram { .. } | ReplyAddress::NyxidRelay { .. } => Self::Chat,
            ReplyAddress::Sync { .. } | ReplyAddress::ResultWebhook { .. } | ReplyAddress::None => {
                Self::Custom
            }
        }
    }
}

impl EffectiveAddress {
    /// The address's class; `None` for a rejected send, which has none.
    pub fn class(&self) -> Option<AddressClass> {
        match self {
            Self::Origin(address) => Some(AddressClass::of(address)),
            Self::Rejected { .. } => None,
        }
    }
}

/// Where a consumer's automatic output (answer or failure text) goes:
/// its own `reply.destination` when that is `default`, else a hand-off's
/// `lineage.reply`, else the origin. Unrouted, always the origin.
pub(crate) fn output_address(
    event: &EventRefPayload,
    destination: ReplyChoice,
    routing_send: bool,
) -> EffectiveAddress {
    if !routing_send {
        return EffectiveAddress::Origin(event.reply.clone());
    }
    let requested = match destination {
        ReplyChoice::Default => ReplyChoice::Default,
        ReplyChoice::Origin => lineage_reply(event),
    };
    resolve(event, requested)
}

/// Where a tool send goes: a hand-off's `lineage.reply`, else the origin.
/// Unrouted, always the origin.
pub(crate) fn tool_address(event: &EventRefPayload, routing_send: bool) -> EffectiveAddress {
    if !routing_send {
        return EffectiveAddress::Origin(event.reply.clone());
    }
    resolve(event, lineage_reply(event))
}

fn lineage_reply(event: &EventRefPayload) -> ReplyChoice {
    event
        .lineage
        .as_ref()
        .map_or(ReplyChoice::Origin, |lineage| lineage.reply)
}

/// The default destination is rejected until a transport can deliver it.
fn resolve(event: &EventRefPayload, requested: ReplyChoice) -> EffectiveAddress {
    match requested {
        ReplyChoice::Origin => EffectiveAddress::Origin(event.reply.clone()),
        ReplyChoice::Default => EffectiveAddress::Rejected {
            reason: DEFAULT_REPLY_UNAVAILABLE,
        },
    }
}

/// Count a send refused because the default destination is unavailable,
/// by the transport the channel's default destination names.
pub(super) fn count_unavailable(def: &ChannelDefinition) {
    let transport = match def
        .routing
        .as_ref()
        .and_then(|routing| routing.default_reply.as_ref())
    {
        Some(DefaultReply::TelegramChat { .. }) => "telegram",
        Some(DefaultReply::RelayConversation { .. }) => "relay",
        Some(DefaultReply::Sinks {}) => "sinks",
        None => "none",
    };
    metrics::counter!(names::DEFAULT_REPLY, names::TRANSPORT => transport,
        names::OUTCOME => "unavailable")
    .increment(1);
}

/// Whether `message` would bind an interactive card to a hand-off. Cards
/// bind actor and chat at their origin (G33), and a hand-off's actor is
/// `consumer:<A>`, so such a card could never be clicked. Only Telegram,
/// native or relayed, renders controls as a card.
pub(crate) fn handoff_controls(event: &EventRefPayload, message: &OutboundMessage) -> bool {
    let telegram = matches!(
        &event.reply,
        ReplyAddress::Telegram { .. }
            | ReplyAddress::NyxidRelay {
                platform: cmaeg_core::channel::Platform::Telegram,
                ..
            }
    );
    telegram && event.lineage.is_some() && crate::interactions::has_controls(message)
}

#[cfg(test)]
#[path = "address_tests.rs"]
mod tests;
