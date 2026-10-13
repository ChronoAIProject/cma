//! The Producers tab: every custom producer the channel has seen, by its
//! stripped `ce.source`, with the advisory note that identity proves only
//! possession of a channel credential.
use super::{activity::launch_label, escape, table, when};
use crate::console::producers::ProducerView;

pub(crate) fn render(producers: &[ProducerView]) -> String {
    let mut html = String::from("<p class=\"notice\" role=\"status\">A producer's identity is advisory: it proves only that the producer holds one of this channel's credentials. Use per-producer keys for hard separation.</p>");
    if producers.is_empty() {
        html.push_str(
            "<p class=\"empty\">No custom producer has been seen since the gateway started.</p>",
        );
        return html;
    }
    let rows: Vec<String> = producers
        .iter()
        .map(|producer| {
            format!(
                "<td><code>{}</code></td><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>",
                escape(&producer.source_uri),
                escape(&producer.hint),
                escape(&when(producer.first_seen)),
                escape(&when(producer.last_seen)),
                producer.count,
                escape(&producer.last_type),
                launch_label(producer.last_outcome),
            )
        })
        .collect();
    html.push_str(&table(
        &[
            "Producer (ce.source)",
            "Credential",
            "First seen",
            "Last seen",
            "Events",
            "Last type",
            "Last outcome",
        ],
        &rows,
    ));
    html
}
