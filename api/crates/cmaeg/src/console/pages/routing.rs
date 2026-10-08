//! The read-only Routing tab: which consumers an event goes to. Routing is
//! changed through the REST API.
use cmaeg_core::{
    channel::{Addressing, AgentEventPolicy},
    ids::ConsumerId,
};

use super::{banners, escape, facts, table};
use crate::console::{
    directory::ChannelView,
    routing_view::{reply_label, RoutingView, RuleView},
    summary::ChannelSummary,
};

pub(crate) fn render(view: &ChannelView, multi_consumer: bool) -> String {
    let summary = &view.summary;
    let receives_all = summary.default_receives_all == Some(true);
    let mut html = banners::routing(summary.routed, receives_all, multi_consumer);
    if !summary.routed {
        let default = summary
            .consumers
            .first()
            .map_or_else(|| "default".to_owned(), |consumer| consumer.label.clone());
        html.push_str("<p class=\"lead\">This channel has no routing policy: every admitted event goes to its default consumer, and its replies go back to the source the event came from.</p>");
        html.push_str(&facts(&[
            ("Model", "legacy".to_owned()),
            ("Default targets", escape(&default)),
            ("Rules", "none".to_owned()),
            ("Default consumer receives every event", "yes".to_owned()),
        ]));
        return html;
    }
    let Some(routing) = summary
        .detail
        .as_ref()
        .and_then(|detail| detail.routing.as_ref())
    else {
        html.push_str("<p class=\"empty\">This channel's definition cannot be read, so its routing is not shown.</p>");
        return html;
    };
    html.push_str("<p class=\"lead\">An event goes to the consumers its explicit addressing names, else to the targets of the first matching active rule, else to the default targets. Rules and default targets skip a consumer that is pending, or that does not answer non-human actors when the actor is not a person; addressing that names such a consumer is refused.</p>");
    html.push_str(&facts(&[
        ("Model", "routed".to_owned()),
        (
            "Default targets",
            consumer_list(summary, &routing.default_targets),
        ),
        (
            "Default consumer receives every event",
            if receives_all { "yes" } else { "no" }.to_owned(),
        ),
        ("Most targets per event", routing.max_targets.to_string()),
        ("Reply labels", reply_label(routing.reply_label).to_owned()),
        (
            "Default reply destination",
            routing
                .default_reply
                .as_ref()
                .map_or_else(|| "none".to_owned(), |reply| escape(&reply.description)),
        ),
        ("Agent events", agent_events(&routing.agent_events)),
    ]));
    html.push_str(&addressing(&routing.addressing));
    html.push_str(&rules(summary, routing));
    html
}

fn addressing(addressing: &Addressing) -> String {
    let on = |value: bool| if value { "on" } else { "off" }.to_owned();
    format!(
        "<h2>Addressing</h2><p class=\"lead\">Mentions override rules, unless the consumer is not mentionable.</p>{}",
        facts(&[
            ("Leading @name mentions", on(addressing.mentions)),
            ("/to command", on(addressing.command)),
            ("cmaegtargets event field", on(addressing.event_field)),
            ("Unknown target notice", on(addressing.unknown_target_notice)),
        ])
    )
}

fn rules(summary: &ChannelSummary, routing: &RoutingView) -> String {
    if routing.rules.is_empty() {
        return "<h2>Rules</h2><p class=\"empty\">No rules: events without addressing go to the default targets.</p>".to_owned();
    }
    let rows: Vec<String> = routing
        .rules
        .iter()
        .enumerate()
        .map(|(index, rule)| rule_row(summary, index, rule))
        .collect();
    format!(
        "<h2>Rules</h2><p class=\"lead\">In order; the first active rule that matches decides. An inactive rule names only sources this channel does not hold.</p>{}",
        table(&["#", "Rule", "Sources", "Matches", "Targets"], &rows)
    )
}

fn rule_row(summary: &ChannelSummary, index: usize, rule: &RuleView) -> String {
    let badge = if rule.active {
        ""
    } else {
        " <span class=\"badge\">inactive</span>"
    };
    let sources = if rule.sources.is_empty() {
        "any".to_owned()
    } else {
        rule.sources
            .iter()
            .map(|id| format!("<code>{}</code>", escape(id.as_str())))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let matches = if rule.matches.is_empty() {
        "every event".to_owned()
    } else {
        escape(&rule.matches.join("; "))
    };
    format!(
        "<td>{}</td><td><code>{}</code>{badge}</td><td>{sources}</td><td>{matches}</td><td>{}</td>",
        index + 1,
        escape(&rule.id),
        consumer_list(summary, &rule.targets),
    )
}

/// Consumers by `@name`, with their id, in the order given.
fn consumer_list(summary: &ChannelSummary, ids: &[ConsumerId]) -> String {
    ids.iter()
        .map(|id| {
            let name = summary
                .consumers
                .iter()
                .find(|consumer| consumer.id == *id)
                .map_or_else(String::new, |consumer| {
                    format!("@{} ", escape(&consumer.name))
                });
            format!("{name}<code>{}</code>", escape(id.as_str()))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn agent_events(policy: &AgentEventPolicy) -> String {
    format!(
        "at most {} hops; {} per consumer and {} per channel a minute",
        policy.max_hops, policy.per_consumer_per_minute, policy.per_channel_per_minute
    )
}
