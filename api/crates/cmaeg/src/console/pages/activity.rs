//! The Activity tab: the newest 50 entries of the channel's activity ring,
//! filterable by consumer, outcome and source, with an optional meta
//! refresh. Entries hold no message text, CloudEvent data or reference.
use cmaeg_core::ids::ConsumerId;

use super::{channel::path, escape, table, when};
use crate::{
    console::{
        activity::{ActivityEntry, ActivityFilter, ConsumerProgress},
        directory::ChannelView,
        observer::DeliveryClass,
    },
    turns::observer::{DeliveryStep, Destination, LaunchClass},
};

/// The refresh intervals the tab offers, in seconds; any other value is
/// ignored.
const REFRESH_SECONDS: [u32; 3] = [10, 30, 60];
/// The longest source filter.
const MAX_SOURCE_BYTES: usize = 64;

/// The refresh interval `value` asks for, when it is one the tab offers.
pub(crate) fn refresh(value: Option<&str>) -> Option<u32> {
    let seconds: u32 = value?.parse().ok()?;
    REFRESH_SECONDS.contains(&seconds).then_some(seconds)
}

/// The head element of a refreshing page.
pub(crate) fn refresh_head(seconds: Option<u32>) -> String {
    seconds.map_or_else(String::new, |seconds| {
        format!("<meta http-equiv=\"refresh\" content=\"{seconds}\">")
    })
}

/// The filter the query asks for; an invalid value filters nothing.
pub(crate) fn filter(
    consumer: Option<&str>,
    outcome: Option<&str>,
    source: Option<&str>,
) -> ActivityFilter {
    ActivityFilter {
        consumer: consumer.and_then(|value| ConsumerId::parse(value).ok()),
        outcome: outcome.and_then(launch_class),
        source: source
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= MAX_SOURCE_BYTES
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || b"_:".contains(&byte))
            })
            .map(str::to_owned),
    }
}

pub(crate) struct ActivityView<'a> {
    pub view: &'a ChannelView,
    pub entries: &'a [ActivityEntry],
    pub filter: &'a ActivityFilter,
    pub refresh: Option<u32>,
}

pub(crate) fn render(activity: &ActivityView<'_>) -> String {
    let mut html = form(activity);
    if activity.entries.is_empty() {
        html.push_str("<p class=\"empty\">No activity in memory. Activity is kept only in this gateway's memory and starts again after a restart.</p>");
        return html;
    }
    let rows: Vec<String> = activity.entries.iter().map(row).collect();
    html.push_str(&table(
        &[
            "Time",
            "Event",
            "Source",
            "Kind",
            "Actor",
            "Chat",
            "Attachments",
            "Consumers",
        ],
        &rows,
    ));
    html
}

fn form(activity: &ActivityView<'_>) -> String {
    let filter = activity.filter;
    let options = |choices: &[(String, String)], chosen: Option<&str>| -> String {
        let mut html = String::from("<option value=\"\">Any</option>");
        for (value, label) in choices {
            let selected = if Some(value.as_str()) == chosen {
                " selected"
            } else {
                ""
            };
            html.push_str(&format!(
                "<option value=\"{}\"{selected}>{}</option>",
                escape(value),
                escape(label)
            ));
        }
        html
    };
    let summary = &activity.view.summary;
    let consumers: Vec<(String, String)> = summary
        .consumers
        .iter()
        .map(|consumer| (consumer.id.as_str().to_owned(), consumer.label.clone()))
        .collect();
    let outcomes: Vec<(String, String)> = ["accepted", "deferred", "refused"]
        .into_iter()
        .map(|value| (value.to_owned(), value.to_owned()))
        .collect();
    let sources: Vec<(String, String)> = summary
        .detail
        .iter()
        .flat_map(|detail| detail.sources.iter())
        .map(|source| (source.activity_label.clone(), source.id.clone()))
        .collect();
    let refresh: Vec<(String, String)> = REFRESH_SECONDS
        .into_iter()
        .map(|seconds| (seconds.to_string(), format!("every {seconds} s")))
        .collect();
    format!(
        "<form class=\"filter\" method=\"get\" action=\"{}\"><input type=\"hidden\" name=\"tab\" value=\"activity\"><label for=\"consumer\">Consumer</label><select id=\"consumer\" name=\"consumer\">{}</select><label for=\"outcome\">Outcome</label><select id=\"outcome\" name=\"outcome\">{}</select><label for=\"source\">Source</label><select id=\"source\" name=\"source\">{}</select><label for=\"refresh\">Refresh</label><select id=\"refresh\" name=\"refresh\">{}</select><button type=\"submit\" class=\"secondary\">Apply</button></form>",
        escape(&path(&activity.view.record)),
        options(&consumers, filter.consumer.as_ref().map(ConsumerId::as_str)),
        options(&outcomes, filter.outcome.map(launch_label)),
        options(&sources, filter.source.as_deref()),
        options(&refresh, activity.refresh.map(|seconds| seconds.to_string()).as_deref()),
    )
}

fn row(entry: &ActivityEntry) -> String {
    let event = entry.event_key.as_deref().map_or_else(
        || "—".to_owned(),
        |key| format!("<code>{}</code>", escape(key)),
    );
    let consumers = if let Some(reason) = &entry.refusal {
        format!("refused: {}", escape(reason))
    } else if entry.consumers.is_empty() {
        "—".to_owned()
    } else {
        entry
            .consumers
            .iter()
            .map(progress_label)
            .collect::<Vec<_>>()
            .join("<br>")
    };
    format!(
        "<td>{}</td><td>{event}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{consumers}</td>",
        escape(&when(entry.at)),
        escape(&entry.source),
        escape(entry.kind),
        or_dash_empty(&entry.actor_masked),
        or_dash_empty(&entry.chat_masked),
        attachments(entry.attachments),
    )
}

/// One consumer's progress on one line, escaped.
pub(crate) fn progress_label(progress: &ConsumerProgress) -> String {
    let mut parts = vec![format!(
        "<code>{}</code>",
        escape(progress.consumer_id.as_str())
    )];
    if let Some(launch) = progress.launch {
        parts.push(launch_label(launch).to_owned());
    }
    if let Some(ms) = progress.accepted_ms {
        parts.push(format!("after {ms} ms"));
    }
    if let Some((step, outcome)) = &progress.delivery {
        parts.push(format!(
            "{} {}",
            step_label(*step),
            escape(&delivery_label(outcome))
        ));
    }
    if let Some(destination) = progress.destination {
        parts.push(format!("to {}", destination_label(destination)));
    }
    parts.join(" · ")
}

pub(crate) fn launch_label(class: LaunchClass) -> &'static str {
    match class {
        LaunchClass::Accepted => "accepted",
        LaunchClass::Deferred => "deferred",
        LaunchClass::Refused => "refused",
    }
}

fn launch_class(value: &str) -> Option<LaunchClass> {
    match value {
        "accepted" => Some(LaunchClass::Accepted),
        "deferred" => Some(LaunchClass::Deferred),
        "refused" => Some(LaunchClass::Refused),
        _ => None,
    }
}

pub(crate) fn step_label(step: DeliveryStep) -> &'static str {
    match step {
        DeliveryStep::Ack => "acknowledgement",
        DeliveryStep::Heartbeat => "heartbeat",
        DeliveryStep::Answer => "answer",
        DeliveryStep::ToolReply => "tool reply",
        DeliveryStep::Failure => "failure notice",
        DeliveryStep::DefaultReply => "default reply",
    }
}

/// A delivery outcome; a rejection carries its bounded snake_case reason.
pub(crate) fn delivery_label(outcome: &DeliveryClass) -> String {
    match outcome {
        DeliveryClass::Confirmed => "delivered".to_owned(),
        DeliveryClass::Unknown => "unconfirmed".to_owned(),
        DeliveryClass::Rejected { reason } => format!("failed ({reason})"),
        DeliveryClass::RateLimited => "rate limited".to_owned(),
        DeliveryClass::Unsupported => "unsupported".to_owned(),
    }
}

pub(crate) fn destination_label(destination: Destination) -> &'static str {
    match destination {
        Destination::Origin => "origin",
        Destination::Default => "default",
    }
}

/// Attachment counts by kind.
pub(crate) fn attachments(counts: [u8; 4]) -> String {
    let parts: Vec<String> = ["image", "audio", "video", "file"]
        .iter()
        .zip(counts)
        .filter(|(_, count)| *count > 0)
        .map(|(kind, count)| format!("{count} {kind}"))
        .collect();
    if parts.is_empty() {
        "—".to_owned()
    } else {
        parts.join(", ")
    }
}

fn or_dash_empty(value: &str) -> String {
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
    fn only_the_offered_refresh_intervals_are_kept() {
        assert_eq!(refresh(Some("10")), Some(10));
        assert_eq!(refresh(Some("30")), Some(30));
        assert_eq!(refresh(Some("60")), Some(60));
        for ignored in ["7", "0", "-10", "10.0", "x", ""] {
            assert_eq!(refresh(Some(ignored)), None, "{ignored}");
        }
        assert_eq!(refresh_head(None), "");
        assert_eq!(
            refresh_head(Some(10)),
            "<meta http-equiv=\"refresh\" content=\"10\">"
        );
    }

    #[test]
    fn invalid_filter_values_filter_nothing() {
        let kept = filter(Some("default"), Some("refused"), Some("nyxid_relay:lark"));
        assert_eq!(kept.consumer, Some(ConsumerId::default_consumer()));
        assert_eq!(kept.outcome, Some(LaunchClass::Refused));
        assert_eq!(kept.source.as_deref(), Some("nyxid_relay:lark"));
        let dropped = filter(Some("<script>"), Some("maybe"), Some("\"><img src=x>"));
        assert!(dropped.consumer.is_none() && dropped.outcome.is_none());
        assert!(dropped.source.is_none());
    }

    #[test]
    fn attachment_counts_name_their_kinds() {
        assert_eq!(attachments([0; 4]), "—");
        assert_eq!(attachments([2, 0, 1, 3]), "2 image, 1 video, 3 file");
    }
}
