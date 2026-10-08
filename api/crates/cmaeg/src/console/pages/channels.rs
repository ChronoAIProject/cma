//! The channel list at `GET /console`: one table row per channel the
//! authorized owner has, 50 to a page, optionally filtered by name.
use super::{banners, escape, when};
use crate::console::{
    directory::{ChannelPage, ChannelRow},
    reads::PAGE_ROWS,
    redirect::HOME,
    source_changes::SourceAvailability,
    summary::ChannelSummary,
};

/// The longest name filter, in characters.
pub(crate) const MAX_QUERY_CHARS: usize = 80;
/// Characters of a record id the list shows.
const SHORT_RECORD: usize = 8;

pub(crate) struct ListView<'a> {
    /// Whose channels these are, unescaped.
    pub owner: &'a str,
    pub page: &'a ChannelPage,
    pub page_no: usize,
    pub query: Option<&'a str>,
    pub sources: SourceAvailability,
    pub multi_consumer: bool,
}

/// The list's main area.
pub(crate) fn body(view: &ListView<'_>) -> String {
    let page = view.page;
    let summaries = || page.rows.iter().filter_map(|row| row.summary.as_deref());
    let routed: Vec<&ChannelSummary> = summaries().filter(|summary| summary.routed).collect();
    let mut html = format!(
        "<h1>Channels</h1><p class=\"lead\">Channels of {}.</p>{}{}{}",
        escape(view.owner),
        banners::index(page.loading, page.partial),
        banners::routing(
            !routed.is_empty(),
            routed
                .iter()
                .all(|summary| summary.default_receives_all == Some(true)),
            view.multi_consumer,
        ),
        banners::sources(view.sources),
    );
    html.push_str(&filter(view.query));
    if page.rows.is_empty() {
        let empty = if view.query.is_some() {
            "No channel name matches this filter."
        } else {
            "No channels to show yet."
        };
        html.push_str(&format!("<p class=\"empty\">{empty}</p>"));
        return html;
    }
    html.push_str("<div class=\"scroll\"><table><thead><tr><th>Name</th><th>Record</th><th>Status</th><th>Sources</th><th>Consumers</th><th>Routing</th><th>Last event</th><th>Last hour</th><th>Version</th></tr></thead><tbody>");
    for row in &page.rows {
        html.push_str(&table_row(row));
    }
    html.push_str("</tbody></table></div>");
    html.push_str(&pager(view));
    html
}

fn filter(query: Option<&str>) -> String {
    format!(
        "<form class=\"filter\" method=\"get\" action=\"{HOME}\"><label for=\"q\">Name contains</label><input id=\"q\" name=\"q\" maxlength=\"{MAX_QUERY_CHARS}\" value=\"{}\"><button type=\"submit\" class=\"secondary\">Filter</button></form>",
        escape(query.unwrap_or_default())
    )
}

fn table_row(row: &ChannelRow) -> String {
    let short: String = row.record_id.chars().take(SHORT_RECORD).collect();
    let summary = row.summary.as_deref();
    let name = match (summary, row.name.is_empty()) {
        (None, _) => "<span class=\"empty\">Loading…</span>".to_owned(),
        (Some(_), true) => "<span class=\"empty\">(no name)</span>".to_owned(),
        (Some(_), false) => escape(&row.name),
    };
    let status = summary.map_or("loading", |summary| status_label(summary));
    let sources = summary.map_or_else(String::new, |summary| {
        summary.sources.iter().map(|id| chip(id, "")).collect()
    });
    let consumers = summary.map_or_else(String::new, |summary| {
        summary
            .consumers
            .iter()
            .map(|consumer| {
                let pending = if consumer.pending {
                    " <span class=\"badge\">Pending</span>"
                } else {
                    ""
                };
                format!(
                    "<span class=\"chip\">{} · {}{pending}</span>",
                    escape(&consumer.label),
                    escape(&consumer.slug)
                )
            })
            .collect()
    });
    let routing = summary.map_or(
        "",
        |summary| {
            if summary.routed {
                "routed"
            } else {
                "legacy"
            }
        },
    );
    let last = row.activity.last_event.map_or_else(|| "—".to_owned(), when);
    format!(
        "<tr><td>{name}</td><td><code>{}</code></td><td>{}</td><td>{sources}</td><td>{consumers}</td><td>{routing}</td><td>{}</td><td>{} events · {} failed</td><td>{}</td></tr>",
        escape(&short),
        chip(status, status),
        escape(&last),
        row.activity.events,
        row.activity.failures,
        row.version,
    )
}

/// A summary's status word: `ready`, `pending`, `invalid`, `unreadable` or
/// `unavailable`.
pub(crate) fn status_label(summary: &ChannelSummary) -> &'static str {
    use crate::console::summary::SummaryStatus::*;
    match summary.status {
        Ready => "ready",
        Pending => "pending",
        Invalid => "invalid",
        Unreadable => "unreadable",
        Unavailable => "unavailable",
    }
}

/// A chip; `class` is a fixed console word.
pub(crate) fn chip(text: &str, class: &str) -> String {
    format!(
        "<span class=\"chip {}\">{}</span>",
        escape(class),
        escape(text)
    )
}

/// Previous and next links that keep the filter.
fn pager(view: &ListView<'_>) -> String {
    let pages = view.page.total.div_ceil(PAGE_ROWS).max(1);
    if pages <= 1 {
        return String::new();
    }
    let link = |page: usize, label: &str| {
        let mut href = format!("{HOME}?page={page}");
        if let Some(query) = view.query {
            let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
            href.push_str(&format!("&q={encoded}"));
        }
        format!(
            "<a class=\"button secondary\" href=\"{}\">{label}</a>",
            escape(&href)
        )
    };
    let mut html = String::from("<nav class=\"pager\">");
    if view.page_no > 0 {
        html.push_str(&link(view.page_no - 1, "Previous"));
    }
    html.push_str(&format!(
        "<span>Page {} of {pages} · {} channels</span>",
        view.page_no + 1,
        view.page.total
    ));
    if view.page_no + 1 < pages {
        html.push_str(&link(view.page_no + 1, "Next"));
    }
    html.push_str("</nav>");
    html
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::console::{
        activity::RecentActivity,
        summary::{ChannelSummary, ConsumerChip, SummaryStatus},
    };

    fn summary(name: &str) -> ChannelSummary {
        ChannelSummary {
            owner: "alice".into(),
            owner_kind: None,
            name: name.into(),
            created_at: String::new(),
            sources: vec!["telegram".into(), "relay_lark".into()],
            consumers: vec![ConsumerChip {
                id: cmaeg_core::ids::ConsumerId::default_consumer(),
                name: "default".into(),
                label: "<b>label</b>".into(),
                slug: "cma_codex".into(),
                key_hash: "sha256:…00000000".into(),
                pending: true,
            }],
            routed: false,
            default_receives_all: None,
            status: SummaryStatus::Ready,
            record_bytes: 100,
            warnings: Vec::new(),
        }
    }

    fn view<'a>(page: &'a ChannelPage, query: Option<&'a str>) -> ListView<'a> {
        ListView {
            owner: "your personal account",
            page,
            page_no: 0,
            query,
            sources: SourceAvailability::Available,
            multi_consumer: false,
        }
    }

    #[test]
    fn rows_show_every_column_escaped() {
        let page = ChannelPage {
            rows: vec![ChannelRow {
                record_id: "00000000-0000-4000-8000-000000000001".into(),
                name: "<script>alert(1)</script>".into(),
                version: 7,
                summary: Some(Arc::new(summary("<script>alert(1)</script>"))),
                activity: RecentActivity {
                    last_event: None,
                    events: 3,
                    failures: 1,
                },
            }],
            total: 1,
            loading: None,
            partial: false,
        };
        let html = body(&view(&page, Some("\"><img src=x>")));
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<img"));
        assert!(html.contains("<code>00000000</code>"));
        assert!(html.contains("relay_lark"));
        assert!(html.contains("&lt;b&gt;label&lt;/b&gt; · cma_codex"));
        assert!(html.contains(">Pending<"));
        assert!(html.contains("3 events · 1 failed"));
        assert!(html.contains("<td>legacy</td>"));
        assert!(html.contains("<td>7</td>"));
    }

    #[test]
    fn banners_render_for_loading_and_partial_indexes() {
        let page = ChannelPage {
            loading: Some(42),
            partial: true,
            ..ChannelPage::default()
        };
        let html = body(&ListView {
            sources: SourceAvailability::Disabled,
            ..view(&page, None)
        });
        assert!(html.contains("Loading channels… 42"));
        assert!(html.contains("Partial index"));
        assert!(html.contains("Source changes are disabled by the operator."));
        assert!(html.contains("No channels to show yet."));
    }

    #[test]
    fn the_pager_keeps_the_filter() {
        let row = ChannelRow {
            record_id: "r".into(),
            name: "n".into(),
            version: 1,
            summary: None,
            activity: RecentActivity::default(),
        };
        let page = ChannelPage {
            rows: vec![row],
            total: 120,
            loading: None,
            partial: false,
        };
        let html = body(&ListView {
            page_no: 1,
            ..view(&page, Some("a&b"))
        });
        assert!(html.contains("href=\"/console?page=0&amp;q=a%26b\""));
        assert!(html.contains("href=\"/console?page=2&amp;q=a%26b\""));
        assert!(html.contains("Page 2 of 3 · 120 channels"));
        assert!(html.contains("Loading…"));
    }
}
