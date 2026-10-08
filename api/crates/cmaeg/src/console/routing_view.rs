//! What the Routing tab and the detach confirmation show of a channel's
//! routing policy, projected when its summary is built and cached with it.
//!
//! Rules keep their sources, kinds, `ce.type` patterns and stripped
//! `ce.source` patterns, but chat and sender lists only by count, and the
//! default reply destination keeps only its source and a masked chat or
//! route: a policy may name people's chats, and the console masks those
//! everywhere.
use cmaeg_core::{
    channel::{
        ActivityKindName, Addressing, AgentEventPolicy, ChannelDefinition, DefaultReply,
        ReplyLabel, RoutingPolicy, RoutingRule, SourceId,
    },
    ids::ConsumerId,
};
use serde::Serialize;

use super::mask;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoutingView {
    /// Where an event no addressing or rule selects goes.
    pub default_targets: Vec<ConsumerId>,
    /// In policy order; the first active match wins.
    pub rules: Vec<RuleView>,
    pub addressing: Addressing,
    pub max_targets: u8,
    pub reply_label: ReplyLabel,
    pub default_reply: Option<DefaultReplyView>,
    pub agent_events: AgentEventPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleView {
    pub id: String,
    /// False when the rule names only sources the channel does not hold, so
    /// it can never match.
    pub active: bool,
    /// The source slots it names; empty matches every source.
    pub sources: Vec<SourceId>,
    /// Its other match lists, described without chat or sender ids.
    pub matches: Vec<String>,
    pub targets: Vec<ConsumerId>,
}

/// The channel's default reply destination, masked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefaultReplyView {
    /// The source it replies through; none for the result webhook sinks.
    pub source: Option<SourceId>,
    pub description: String,
}

impl RoutingView {
    /// The policy of `definition`, if it has one.
    pub fn of(definition: &ChannelDefinition) -> Option<Self> {
        let policy: &RoutingPolicy = definition.routing.as_ref()?;
        Some(Self {
            default_targets: policy.default_targets.clone(),
            rules: policy
                .rules
                .iter()
                .map(|rule| RuleView::of(rule, definition))
                .collect(),
            addressing: policy.addressing.clone(),
            max_targets: policy.max_targets,
            reply_label: policy.reply_label,
            default_reply: policy.default_reply.as_ref().map(DefaultReplyView::of),
            agent_events: policy.agent_events.clone(),
        })
    }

    /// The rules active now that detaching `source` leaves without any
    /// attached source, given the channel's attached `sources`.
    pub fn inert_after_detach<'a>(
        &'a self,
        source: &'a SourceId,
        sources: &'a [SourceId],
    ) -> impl Iterator<Item = &'a RuleView> {
        self.rules.iter().filter(move |rule| {
            rule.active
                && rule.sources.contains(source)
                && rule
                    .sources
                    .iter()
                    .all(|named| named == source || !sources.contains(named))
        })
    }
}

impl RuleView {
    fn of(rule: &RoutingRule, definition: &ChannelDefinition) -> Self {
        let when = &rule.when;
        let mut matches = Vec::new();
        if !when.kinds.is_empty() {
            let kinds: Vec<&str> = when.kinds.iter().map(|kind| kind_label(*kind)).collect();
            matches.push(format!("kinds {}", kinds.join(", ")));
        }
        if !when.chats.is_empty() {
            matches.push(format!("{} chat(s)", when.chats.len()));
        }
        if !when.senders.is_empty() {
            matches.push(format!("{} sender(s)", when.senders.len()));
        }
        if !when.event_types.is_empty() {
            let types: Vec<&str> = when
                .event_types
                .iter()
                .map(|value| value.as_str())
                .collect();
            matches.push(format!("event types {}", types.join(", ")));
        }
        if !when.event_sources.is_empty() {
            let sources: Vec<String> = when
                .event_sources
                .iter()
                .map(|value| mask::source_uri(value.as_str()))
                .collect();
            matches.push(format!("event sources {}", sources.join(", ")));
        }
        Self {
            id: rule.id.as_str().to_owned(),
            active: rule.is_active(definition),
            sources: when.sources.clone(),
            matches,
            targets: rule.targets.clone(),
        }
    }
}

impl DefaultReplyView {
    fn of(reply: &DefaultReply) -> Self {
        match reply {
            DefaultReply::TelegramChat {
                source_id,
                chat_id,
                thread_id,
            } => Self {
                source: Some(source_id.clone()),
                description: format!(
                    "chat {}{} of {}",
                    mask::chat(chat_id),
                    if thread_id.is_some() {
                        ", in a thread"
                    } else {
                        ""
                    },
                    source_id.as_str()
                ),
            },
            DefaultReply::RelayConversation {
                source_id,
                route_id,
            } => Self {
                source: Some(source_id.clone()),
                description: format!("route {} of {}", mask::chat(route_id), source_id.as_str()),
            },
            DefaultReply::Sinks {} => Self {
                source: None,
                description: "the channel's result webhook sinks".to_owned(),
            },
        }
    }
}

pub fn reply_label(label: ReplyLabel) -> &'static str {
    match label {
        ReplyLabel::Auto => "auto",
        ReplyLabel::Always => "always",
        ReplyLabel::Never => "never",
    }
}

fn kind_label(kind: ActivityKindName) -> &'static str {
    match kind {
        ActivityKindName::Message => "message",
        ActivityKindName::Command => "command",
        ActivityKindName::Interaction => "interaction",
        ActivityKindName::Event => "event",
    }
}
