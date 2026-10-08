//! The read-only Consumers tab: the agents that consume the channel's
//! events. Consumers are edited through the REST API.
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
                "Reply"
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
        "<td>{}{badges}<br><span class=\"empty\">@{} · <code>{}</code></span></td><td>{}</td><td>{}</td><td><code>{}</code></td><td>{}</td><td><code>{}</code></td><td>automatic final answer {}</td>",
        escape(&consumer.label),
        escape(&consumer.name),
        escape(consumer.id.as_str()),
        escape(&consumer.slug),
        escape(&consumer.target),
        escape(&consumer.binding_id),
        escape(consumer.level),
        escape(&consumer.key_hash),
        if consumer.auto_final { "on" } else { "off" },
    )
}
