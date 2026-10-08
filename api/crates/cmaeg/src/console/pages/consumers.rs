//! The read-only Consumers tab: the agents that consume the channel's
//! events, with what routing reads of each. Consumers are edited through
//! the REST API.
use cmaeg_core::{channel::ReplyChoice, ids::ConsumerId};

use super::{escape, table};
use crate::console::{directory::ChannelView, summary::ConsumerChip};

pub(crate) fn render(view: &ChannelView) -> String {
    let summary = &view.summary;
    if summary.consumers.is_empty() {
        return "<p class=\"empty\">No consumer is known for this channel.</p>".to_owned();
    }
    let rows: Vec<String> = summary
        .consumers
        .iter()
        .map(|consumer| row(consumer, &summary.warnings))
        .collect();
    format!(
        "<p class=\"lead\">Consumers are read-only here; change them through the management API.</p>{}",
        table(
            &[
                "Consumer",
                "Provider",
                "Target",
                "Binding",
                "Level",
                "Agent key",
                "Addressing",
                "Non-human actors",
                "Reply",
                "Publish"
            ],
            &rows
        )
    )
}

fn row(consumer: &ConsumerChip, warnings: &[&str]) -> String {
    let mut badges = String::new();
    if consumer.pending {
        badges.push_str(" <span class=\"badge\">Pending</span>");
    }
    for warning in warnings {
        badges.push_str(&format!(
            " <span class=\"badge warning\">{}</span>",
            escape(&warning.replace('_', " "))
        ));
    }
    format!(
        "<td>{}{badges}<br><span class=\"empty\">@{} · <code>{}</code></span></td><td>{}</td><td>{}</td><td><code>{}</code></td><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td><td>automatic final answer {} · to {}</td><td>{}</td>",
        escape(&consumer.label),
        escape(&consumer.name),
        escape(consumer.id.as_str()),
        escape(&consumer.slug),
        escape(&consumer.target),
        escape(&consumer.binding_id),
        escape(consumer.level),
        escape(&consumer.key_hash),
        if consumer.mentionable {
            "mentionable"
        } else {
            "not mentionable"
        },
        non_human(consumer),
        if consumer.auto_final { "on" } else { "off" },
        destination(consumer.destination),
        publish(consumer.publish.as_deref()),
    )
}

/// The effective capability and where it comes from.
fn non_human(consumer: &ConsumerChip) -> &'static str {
    match (consumer.non_human, consumer.non_human_set) {
        (true, true) => "answers (set on the consumer)",
        (true, false) => "answers (provider default)",
        (false, true) => "ignores (set on the consumer)",
        (false, false) => "ignores (provider default)",
    }
}

pub(crate) fn destination(choice: ReplyChoice) -> &'static str {
    match choice {
        ReplyChoice::Origin => "origin",
        ReplyChoice::Default => "default reply destination",
    }
}

/// Whether the consumer may publish hand-offs, and to whom.
fn publish(targets: Option<&[ConsumerId]>) -> String {
    match targets {
        None => "off".to_owned(),
        Some([]) => "to any routable consumer".to_owned(),
        Some(targets) => format!(
            "to {}",
            targets
                .iter()
                .map(|id| format!("<code>{}</code>", escape(id.as_str())))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
