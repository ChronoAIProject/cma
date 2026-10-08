//! Which sends are routed, and where a routed send goes. A routed send may
//! carry a label and resolve a destination; an unrouted one is exactly
//! today's: no label, no default destination, the current branch.
use crate::telemetry::names;
use cmaeg_core::{
    channel::{ChannelDefinition, DefaultReply, Platform, ReplyChoice, Sink, Source, SourceId},
    message::OutboundMessage,
    seal::{EventRefPayload, MessageRefPayload, ReplyAddress},
};

/// The refusal of a send addressed to the channel's default destination
/// when the snapshot the send carries cannot resolve one.
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

/// Whether a routed send may resolve the channel's default destination:
/// the send is routed, the multi-consumer flag is on, and the snapshot names
/// a `routing.default_reply`. With the flag off it is false, so a send that
/// asks for the default is refused rather than redirected to its origin.
pub(crate) fn default_ok(
    def: &ChannelDefinition,
    payload: &EventRefPayload,
    multi_consumer_enabled: bool,
    kind: SendKind,
) -> bool {
    routing_send(def, payload, multi_consumer_enabled, kind)
        && multi_consumer_enabled
        && def
            .routing
            .as_ref()
            .is_some_and(|routing| routing.default_reply.is_some())
}

/// Where a send goes once its destination is resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectiveAddress {
    /// The sealed address of the event's origin.
    Origin(ReplyAddress),
    /// A chat of the channel's native Telegram bot.
    DefaultTelegramChat {
        bot_id: String,
        chat_id: String,
        thread_id: Option<String>,
    },
    /// A conversation of a NyxID relay source, sent to proactively.
    DefaultRelayConversation {
        platform: Platform,
        route_id: String,
    },
    /// The channel's result webhook sinks.
    DefaultSinks,
    /// Nothing may be sent; an output for a private destination is never
    /// redirected to a possibly public origin.
    Rejected { reason: &'static str },
}

/// The channel's default destination as the snapshot the command carries
/// defines it now: the turn's for automatic outputs, the call's for tools.
/// Never sealed, so changing `routing.default_reply` or detaching its source
/// applies to every later turn and tool call. `None` when nothing is
/// configured, or when the configured source, route, credential or sink is
/// gone from the snapshot.
fn resolve_now(def: &ChannelDefinition) -> Option<EffectiveAddress> {
    let reply = def.routing.as_ref()?.default_reply.as_ref()?;
    let attached = |id: &SourceId| def.sources.iter().find(|source| &source.source_id() == id);
    match reply {
        DefaultReply::TelegramChat {
            source_id,
            chat_id,
            thread_id,
        } => {
            let Some(Source::Telegram { bot_id, .. }) = attached(source_id) else {
                return None;
            };
            def.secrets.telegram_bot_token.as_ref()?;
            Some(EffectiveAddress::DefaultTelegramChat {
                bot_id: bot_id.clone(),
                chat_id: chat_id.clone(),
                thread_id: thread_id.clone(),
            })
        }
        DefaultReply::RelayConversation {
            source_id,
            route_id,
        } => {
            let Some(Source::NyxidRelay {
                platform,
                route_ids,
                ..
            }) = attached(source_id)
            else {
                return None;
            };
            (route_ids.contains(route_id) && def.secrets.channel_key.is_some()).then(|| {
                EffectiveAddress::DefaultRelayConversation {
                    platform: *platform,
                    route_id: route_id.clone(),
                }
            })
        }
        DefaultReply::Sinks {} => has_result_webhook(def).then_some(EffectiveAddress::DefaultSinks),
    }
}

fn has_result_webhook(def: &ChannelDefinition) -> bool {
    def.reply
        .sinks
        .iter()
        .any(|sink| matches!(sink, Sink::ResultWebhook { .. }))
}

/// Whether a send to the event's origin can be delivered: a chat, a result
/// webhook, a Sync origin whose waiter is still registered, or no origin on
/// a channel with a result webhook sink.
fn reply_capable(def: &ChannelDefinition, origin: &ReplyAddress, sync_registered: bool) -> bool {
    match origin {
        ReplyAddress::Telegram { .. }
        | ReplyAddress::NyxidRelay { .. }
        | ReplyAddress::ResultWebhook { .. } => true,
        ReplyAddress::Sync { .. } => sync_registered,
        ReplyAddress::None => has_result_webhook(def),
    }
}

/// The destination a send asks for, in precedence order: `default` when the
/// consumer's own `reply.destination` is `default` (automatic outputs; tools
/// pass `Origin`), else a hand-off's `lineage.reply`, else the origin.
pub(crate) fn requested(consumer: ReplyChoice, event: &EventRefPayload) -> ReplyChoice {
    if consumer == ReplyChoice::Default {
        return ReplyChoice::Default;
    }
    event
        .lineage
        .as_ref()
        .map_or(ReplyChoice::Origin, |lineage| lineage.reply)
}

/// Where a send asking for `requested` goes. Unrouted, always the origin,
/// exactly as today. Routed:
/// - `origin` goes to a reply-capable origin; otherwise to the default
///   destination when `default_ok`; otherwise to the origin as today
///   (`unsupported`, or the sinks);
/// - `default` goes to the default destination when `default_ok`, and is
///   otherwise rejected as `default_reply_unavailable`.
pub(crate) fn effective(
    def: &ChannelDefinition,
    payload: &EventRefPayload,
    requested: ReplyChoice,
    sync_registered: bool,
    multi_consumer_enabled: bool,
    kind: SendKind,
) -> EffectiveAddress {
    let origin = || EffectiveAddress::Origin(payload.reply.clone());
    if !routing_send(def, payload, multi_consumer_enabled, kind) {
        return origin();
    }
    let default = || {
        default_ok(def, payload, multi_consumer_enabled, kind)
            .then(|| resolve_now(def))
            .flatten()
    };
    match requested {
        ReplyChoice::Origin if reply_capable(def, &payload.reply, sync_registered) => origin(),
        ReplyChoice::Origin => default().unwrap_or_else(origin),
        ReplyChoice::Default => default().unwrap_or(EffectiveAddress::Rejected {
            reason: DEFAULT_REPLY_UNAVAILABLE,
        }),
    }
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
    /// The address's class; `None` for a rejected send, which has none. A
    /// default chat is chat-class whatever the event's origin, so a custom
    /// event whose default is a Telegram chat reaches that chat.
    pub fn class(&self) -> Option<AddressClass> {
        match self {
            Self::Origin(address) => Some(AddressClass::of(address)),
            Self::DefaultTelegramChat { .. } | Self::DefaultRelayConversation { .. } => {
                Some(AddressClass::Chat)
            }
            Self::DefaultSinks => Some(AddressClass::Custom),
            Self::Rejected { .. } => None,
        }
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
        Some(DefaultReply::RelayConversation { .. }) => "nyxid_relay",
        Some(DefaultReply::Sinks {}) => "webhook",
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
#[path = "address_table_tests.rs"]
mod table_tests;
#[cfg(test)]
#[path = "address_tests.rs"]
mod tests;
