//! One event's page at `GET /console/channels/{record_id}/events/{key}`:
//! how it was routed, each consumer's timeline and where it came from. It
//! never shows text, CloudEvent data or a reference: the ring holds none.
use cmaeg_core::ids::RecordId;

use super::{
    activity::{attachments, delivery_label, destination_label, launch_label, step_label},
    channel::{href, path, Tab},
    escape, facts, route, when,
};
use crate::console::{
    activity::{ActivityEntry, ConsumerProgress},
    directory::ChannelView,
};

/// The route template of an event's page.
pub(crate) const PATH: &str = "/console/channels/{record_id}/events/{event_key}";
/// Hex characters of an event key.
const EVENT_KEY_HEX: usize = 16;

/// Whether `value` has an event key's shape: 16 lowercase hex characters.
pub(crate) fn valid_key(value: &str) -> bool {
    value.len() == EVENT_KEY_HEX
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The same-origin address of `record`'s event `key`.
pub(crate) fn event_href(record: &RecordId, key: &str) -> String {
    format!("{}/events/{key}", path(record))
}

pub(crate) struct EventView<'a> {
    pub view: &'a ChannelView,
    pub entry: &'a ActivityEntry,
    /// Whether the ring still holds the parent of a published event.
    pub parent_held: bool,
}

pub(crate) fn render(event: &EventView<'_>) -> String {
    let entry = event.entry;
    let record = &event.view.record;
    let key = entry.event_key.as_deref().unwrap_or("—");
    let mut html = format!(
        "<p><a href=\"{}\">Back to activity</a></p><h1>Event <code>{}</code></h1><p class=\"lead\">On channel {}. Only what the gateway observed is shown; message text, event data and references are never kept.</p>",
        escape(&href(record, Tab::Activity)),
        escape(key),
        escape(&super::channel::title(event.view)),
    );
    html.push_str(&facts(&[
        ("Received", escape(&when(entry.at))),
        ("Source", escape(&entry.source)),
        ("Kind", escape(entry.kind)),
        ("Actor", dash(&entry.actor_masked)),
        ("Chat", dash(&entry.chat_masked)),
        ("Attachments", attachments(entry.attachments)),
    ]));
    html.push_str("<h2>Routing</h2>");
    html.push_str(&route::section(entry, &event.view.summary));
    html.push_str("<h2>Consumers</h2>");
    if entry.consumers.is_empty() {
        html.push_str("<p class=\"empty\">No consumer has reported progress for this event.</p>");
    } else {
        for progress in &entry.consumers {
            html.push_str(&timeline(progress));
        }
    }
    if let Some(causation) = &entry.causation {
        let parent = if event.parent_held {
            format!(
                "<a href=\"{}\"><code>{}</code></a>",
                escape(&event_href(record, &causation.parent_key)),
                escape(&causation.parent_key)
            )
        } else {
            format!(
                "<code>{}</code> (no longer in memory)",
                escape(&causation.parent_key)
            )
        };
        html.push_str("<h2>Causation</h2>");
        html.push_str(&facts(&[
            (
                "Published by",
                format!("<code>{}</code>", escape(causation.published_by.as_str())),
            ),
            ("Parent event", parent),
            ("Hop", causation.hop.to_string()),
        ]));
    }
    html
}

/// One consumer's steps in order: launched, accepted after X ms, its latest
/// delivery and where that went.
fn timeline(progress: &ConsumerProgress) -> String {
    let mut steps = Vec::new();
    if let Some(launch) = progress.launch {
        steps.push(format!("launch {}", launch_label(launch)));
    }
    if let Some(ms) = progress.accepted_ms {
        steps.push(format!("accepted after {ms} ms"));
    }
    if progress.tool_replied {
        steps.push("tool reply delivered".to_owned());
    }
    if let Some((step, outcome)) = &progress.delivery {
        steps.push(format!(
            "{} {}",
            step_label(*step),
            escape(&delivery_label(outcome))
        ));
    }
    if let Some(destination) = progress.destination {
        steps.push(format!("destination {}", destination_label(destination)));
    }
    let items: String = steps
        .iter()
        .map(|step| format!("<li>{step}</li>"))
        .collect();
    format!(
        "<h3><code>{}</code></h3><ol class=\"timeline\">{items}</ol>",
        escape(progress.consumer_id.as_str())
    )
}

fn dash(value: &str) -> String {
    if value.is_empty() {
        "—".to_owned()
    } else {
        escape(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_keys_are_sixteen_lowercase_hex_characters() {
        assert!(valid_key("0123456789abcdef"));
        for invalid in [
            "0123456789ABCDEF",
            "0123456789abcde",
            "0123456789abcdef0",
            "<script>alert(1)",
            "",
        ] {
            assert!(!valid_key(invalid), "{invalid}");
        }
    }
}
