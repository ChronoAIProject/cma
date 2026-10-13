//! What the Routing tab and the detach confirmation show of a channel's
//! routing policy, projected when its summary is built and cached with it.
//!
//! Rules keep their distinct sources and kinds, and their `ce.type` and
//! stripped `ce.source` patterns up to [`PATTERN_TEXT_BYTES`] per list with
//! the rest counted, but chat and sender lists only by count. The default
//! reply destination keeps only its source and a masked chat or route: a
//! policy may name people's chats, and the console masks those everywhere.
//!
//! The projection is cached with the channel's summary, so it must stay
//! small however much the owner writes: a stored policy may hold 32 rules of
//! 64 patterns of 512 bytes each, which shown whole would be most of a
//! 64 KiB record per cached summary.
use cmaeg_core::{
    channel::{
        ActivityKindName, Addressing, AgentEventPolicy, ChannelDefinition, DefaultReply,
        ReplyLabel, RoutingPolicy, RoutingRule, SourceId,
    },
    ids::ConsumerId,
};
use serde::Serialize;

use super::mask;

/// Bytes of one rule's pattern list the projection keeps as text; enough for
/// a few typical patterns, whatever a stored list holds.
pub const PATTERN_TEXT_BYTES: usize = 128;

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
    /// The distinct source slots it names, in policy order; empty matches
    /// every source.
    pub sources: Vec<SourceId>,
    /// Its other match lists, described without chat or sender ids and with
    /// each pattern list bounded by [`PATTERN_TEXT_BYTES`].
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
            let kinds: Vec<&str> = distinct(&when.kinds)
                .into_iter()
                .map(|kind| kind_label(*kind))
                .collect();
            matches.push(format!("kinds {}", kinds.join(", ")));
        }
        if !when.chats.is_empty() {
            matches.push(format!("{} chat(s)", when.chats.len()));
        }
        if !when.senders.is_empty() {
            matches.push(format!("{} sender(s)", when.senders.len()));
        }
        if !when.event_types.is_empty() {
            matches.push(pattern_list(
                "event type",
                when.event_types
                    .iter()
                    .map(|value| value.as_str().to_owned()),
            ));
        }
        if !when.event_sources.is_empty() {
            matches.push(pattern_list(
                "event source",
                when.event_sources
                    .iter()
                    .map(|value| mask::source_uri(value.as_str())),
            ));
        }
        Self {
            id: rule.id.as_str().to_owned(),
            active: rule.is_active(definition),
            sources: distinct(&when.sources).into_iter().cloned().collect(),
            matches,
            targets: rule.targets.clone(),
        }
    }
}

/// The first occurrence of each value, in order. A stored list may repeat a
/// source or kind up to its 64-entry bound, which says nothing more than
/// naming it once.
fn distinct<T: PartialEq>(values: &[T]) -> Vec<&T> {
    let mut kept: Vec<&T> = Vec::new();
    for value in values {
        if !kept.contains(&value) {
            kept.push(value);
        }
    }
    kept
}

/// `<noun>s a, b +N more`: whole patterns, in order, while their text fits in
/// [`PATTERN_TEXT_BYTES`], and the rest counted; `N <noun>(s)` when not even
/// the first fits. A pattern is never cut, so what is shown is always a
/// pattern the rule really holds.
fn pattern_list(noun: &str, values: impl ExactSizeIterator<Item = String>) -> String {
    let total = values.len();
    let mut shown: Vec<String> = Vec::new();
    let mut bytes = 0;
    for value in values {
        let separator = if shown.is_empty() { 0 } else { ", ".len() };
        if bytes + separator + value.len() > PATTERN_TEXT_BYTES {
            break;
        }
        bytes += separator + value.len();
        shown.push(value);
    }
    let rest = total - shown.len();
    if shown.is_empty() {
        format!("{total} {noun}(s)")
    } else if rest == 0 {
        format!("{noun}s {}", shown.join(", "))
    } else {
        format!("{noun}s {} +{rest} more", shown.join(", "))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, active: bool, sources: &[&str]) -> RuleView {
        RuleView {
            id: id.to_owned(),
            active,
            sources: sources
                .iter()
                .map(|source| SourceId::parse(source).unwrap())
                .collect(),
            matches: Vec::new(),
            targets: vec![ConsumerId::default_consumer()],
        }
    }

    #[test]
    fn only_active_rules_left_without_an_attached_source_become_inert() {
        let view = RoutingView {
            default_targets: vec![ConsumerId::default_consumer()],
            rules: vec![
                rule("lark-only", true, &["relay_lark"]),
                rule("lark-or-telegram", true, &["relay_lark", "telegram"]),
                rule("lark-or-slack", true, &["relay_lark", "relay_slack"]),
                rule("slack-only", false, &["relay_slack"]),
                rule("any-source", true, &[]),
            ],
            addressing: Addressing::default(),
            max_targets: 4,
            reply_label: ReplyLabel::Auto,
            default_reply: None,
            agent_events: AgentEventPolicy::default(),
        };
        let lark = SourceId::parse("relay_lark").unwrap();
        let attached = [lark.clone(), SourceId::parse("telegram").unwrap()];
        let inert: Vec<&str> = view
            .inert_after_detach(&lark, &attached)
            .map(|rule| rule.id.as_str())
            .collect();
        assert_eq!(inert, ["lark-only", "lark-or-slack"]);
    }

    fn patterns(count: usize, bytes: usize) -> impl ExactSizeIterator<Item = String> {
        (0..count).map(move |index| format!("{index:0>bytes$}"))
    }

    #[test]
    fn short_pattern_lists_are_shown_whole() {
        let shown = pattern_list(
            "event type",
            ["com.example.ci.*", "com.example.cd"]
                .map(str::to_owned)
                .into_iter(),
        );
        assert_eq!(shown, "event types com.example.ci.*, com.example.cd");
    }

    #[test]
    fn long_pattern_lists_keep_whole_patterns_within_their_budget() {
        // Three 40-byte patterns and their separators take 124 bytes; a
        // fourth would not fit.
        let shown = pattern_list("event source", patterns(64, 40));
        assert!(shown.ends_with(" +61 more"), "{shown}");
        assert_eq!(shown.matches(", ").count(), 2, "{shown}");
        assert!(shown.len() <= "event sources ".len() + PATTERN_TEXT_BYTES + " +61 more".len());
        // A list whose first pattern alone exceeds the budget is counted.
        assert_eq!(
            pattern_list("event type", patterns(64, 512)),
            "64 event type(s)"
        );
        assert_eq!(
            pattern_list("event type", patterns(1, PATTERN_TEXT_BYTES + 1)),
            "1 event type(s)"
        );
        assert_eq!(
            pattern_list("event type", patterns(1, PATTERN_TEXT_BYTES)),
            format!("event types {:0>PATTERN_TEXT_BYTES$}", 0)
        );
    }

    #[test]
    fn repeated_sources_and_kinds_are_named_once_in_order() {
        let sources: Vec<SourceId> = ["custom", "relay_lark", "custom", "relay_lark", "telegram"]
            .iter()
            .map(|source| SourceId::parse(source).unwrap())
            .collect();
        let kept: Vec<&str> = distinct(&sources)
            .into_iter()
            .map(SourceId::as_str)
            .collect();
        assert_eq!(kept, ["custom", "relay_lark", "telegram"]);
        let kinds = [ActivityKindName::Event; 64];
        assert_eq!(distinct(&kinds), [&ActivityKindName::Event]);
    }
}
