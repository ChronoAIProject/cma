//! The Sources tab: one row per source with its settings and last event,
//! and, while source changes are available, a Detach control per row and
//! the attach forms. Otherwise a banner says why they are absent.
use time::OffsetDateTime;

use super::{banners, detach::detach_href, escape, or_dash, table, when};
use crate::console::{
    detail::SourceView, directory::ChannelView, source_changes::SourceAvailability,
};

pub(crate) struct SourcesView<'a> {
    pub view: &'a ChannelView,
    /// When each source's newest activity entry was recorded, in source
    /// order.
    pub last_events: &'a [Option<OffsetDateTime>],
    pub availability: SourceAvailability,
    /// The attach forms, shown while source changes are available.
    pub forms: &'a str,
}

pub(crate) fn render(sources: &SourcesView<'_>) -> String {
    let Some(detail) = &sources.view.summary.detail else {
        return "<p class=\"empty\">This channel's definition cannot be read, so its sources are not shown.</p>".to_owned();
    };
    let mut html = String::new();
    if detail.connection_managed {
        html.push_str(
            "<p class=\"notice\" role=\"status\">A connection manages this channel's sources.</p>",
        );
    }
    // A connection-managed channel's sources change only through it.
    let controls =
        sources.availability == SourceAvailability::Available && !detail.connection_managed;
    let record = &sources.view.record;
    if detail.sources.is_empty() {
        html.push_str("<p class=\"empty\">This channel has no sources.</p>");
    } else {
        let rows: Vec<String> = detail
            .sources
            .iter()
            .zip(sources.last_events.iter().chain(std::iter::repeat(&None)))
            .map(|(source, last)| {
                let detach = if controls {
                    format!(
                        "<td><a href=\"{}\">Detach</a></td>",
                        escape(&detach_href(record, &source.id))
                    )
                } else {
                    String::new()
                };
                format!("{}{detach}", row(source, *last))
            })
            .collect();
        let mut headers = vec![
            "Source",
            "Type",
            "Platform",
            "Routes",
            "Bot",
            "Admission",
            "Authentication",
            "Settings",
            "Last event",
        ];
        if controls {
            headers.push("");
        }
        html.push_str(&table(&headers, &rows));
    }
    if controls {
        html.push_str(sources.forms);
    } else {
        html.push_str(&banners::sources(sources.availability));
    }
    html
}

fn row(source: &SourceView, last: Option<OffsetDateTime>) -> String {
    let bot = match (&source.bot_username, &source.bot_id) {
        (None, None) => "—".to_owned(),
        (username, id) => format!(
            "{} · id {}",
            username
                .as_deref()
                .map_or_else(|| "—".to_owned(), |name| format!("@{}", escape(name))),
            or_dash(id.as_deref())
        ),
    };
    let keys = if source.key_ids.is_empty() {
        String::new()
    } else {
        format!(
            "<br><span class=\"empty\">keys</span> {}",
            source
                .key_ids
                .iter()
                .map(|key| format!("<code>{}</code>", escape(key)))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    format!(
        "<td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{bot}</td><td>{}</td><td>{}{keys}</td><td>{}</td><td>{}</td>",
        escape(&source.id),
        escape(source.kind),
        or_dash(source.platform),
        source
            .route_count
            .map_or_else(|| "—".to_owned(), |count| count.to_string()),
        or_dash(source.admission.as_deref()),
        escape(&source.auth),
        if source.settings.is_empty() {
            "—".to_owned()
        } else {
            escape(&source.settings.join("; "))
        },
        last.map_or_else(|| "—".to_owned(), |at| escape(&when(at))),
    )
}
