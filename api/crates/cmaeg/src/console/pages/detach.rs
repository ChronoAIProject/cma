//! The confirmation page before a source is detached, and the page that
//! says a source change cannot be made in this gateway now.
use axum::http::StatusCode;
use cmaeg_core::{channel::SourceId, ids::RecordId};

use super::{
    channel::{href, path, Tab},
    escape, facts,
};
use crate::console::{
    detail::SourceView, directory::ChannelView, source_changes::SourceAvailability,
};

/// The route template of the confirmation page and of its form.
pub(crate) const DETACH_PATH: &str = "/console/channels/{record_id}/sources/{source_id}/detach";

/// The same-origin address of `source`'s detach confirmation.
pub(crate) fn detach_href(record: &RecordId, source: &SourceId) -> String {
    format!("{}/sources/{}/detach", path(record), source.as_str())
}

pub(crate) struct DetachView<'a> {
    pub view: &'a ChannelView,
    pub source: &'a SourceView,
    pub csrf: &'a str,
    pub error: Option<&'a str>,
}

/// What the detach will do, and the form that does it at the version the
/// owner sees.
pub(crate) fn render(detach: &DetachView<'_>) -> String {
    let view = detach.view;
    let source = detach.source;
    let remaining = view
        .summary
        .detail
        .as_ref()
        .map_or(0, |detail| detail.sources.len().saturating_sub(1));
    let error = detach.error.map_or_else(String::new, |message| {
        format!(
            "<p class=\"notice error\" role=\"alert\">{}</p>",
            escape(message)
        )
    });
    let mut consequences = vec![
        "<li>The source stops admitting events as soon as the detach completes.</li>".to_owned(),
        "<li>Secrets only this source uses are dropped; secrets other sources or sinks still use are kept.</li>".to_owned(),
    ];
    if remaining == 0 {
        consequences.push("<li>The channel will have no sources and will admit no events until one is attached.</li>".to_owned());
    }
    format!(
        "<h2>Detach <code>{}</code>?</h2>{error}{}{}<h3>What happens</h3><ul>{}</ul><form class=\"inline\" method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"expected_version\" value=\"{}\"><button type=\"submit\">Detach</button></form> <a class=\"button secondary\" href=\"{}\">Cancel</a>",
        escape(source.id.as_str()),
        facts(&[
            ("Type", escape(source.kind)),
            ("Platform", super::or_dash(source.platform)),
            ("Version", view.version.to_string()),
        ]),
        references(view, source),
        consequences.concat(),
        escape(&detach_href(&view.record, &source.id)),
        escape(detach.csrf),
        view.version,
        escape(&href(&view.record, Tab::Sources)),
    )
}

/// What in the routing policy names `source`: a default reply destination
/// that blocks the detach, and the rules that only it keeps active.
fn references(view: &ChannelView, source: &SourceView) -> String {
    let Some(detail) = view.summary.detail.as_ref() else {
        return String::new();
    };
    let Some(routing) = detail.routing.as_ref() else {
        return "<h3>Blocking references</h3><p>None: this channel has no routing policy, so no default reply source names this source.</p><h3>Rules that become inactive</h3><p>None.</p>".to_owned();
    };
    let blocking = match &routing.default_reply {
        Some(reply) if reply.source.as_ref() == Some(&source.id) => format!(
            "<ul><li>The default reply destination, {}, replies through this source. The detach is refused with 409 until the routing policy names another destination.</li></ul>",
            escape(&reply.description)
        ),
        _ => "<p>None.</p>".to_owned(),
    };
    let attached: Vec<SourceId> = detail.sources.iter().map(|item| item.id.clone()).collect();
    let inert: Vec<String> = routing
        .inert_after_detach(&source.id, &attached)
        .map(|rule| format!("<li><code>{}</code></li>", escape(&rule.id)))
        .collect();
    let inert = if inert.is_empty() {
        "<p>None.</p>".to_owned()
    } else {
        format!(
            "<p>These rules name no other attached source, so they stop matching until such a source is attached again:</p><ul>{}</ul>",
            inert.concat()
        )
    };
    format!("<h3>Blocking references</h3>{blocking}<h3>Rules that become inactive</h3>{inert}")
}

/// The status and text of a source change this gateway cannot make now.
pub(crate) fn unavailable(availability: SourceAvailability) -> (StatusCode, &'static str) {
    match availability {
        SourceAvailability::Disabled => (
            StatusCode::BAD_REQUEST,
            "Source changes are disabled by the operator.",
        ),
        SourceAvailability::NotYetAvailable | SourceAvailability::Available => (
            StatusCode::NOT_IMPLEMENTED,
            "Source changes are not available in this gateway release yet. Nothing was changed.",
        ),
    }
}

/// The body of that page, with a way back to the sources.
pub(crate) fn unavailable_body(record: &RecordId, text: &str) -> String {
    format!(
        "<h1>Source change not made</h1><p class=\"lead\">{}</p><p><a class=\"button secondary\" href=\"{}\">Back to sources</a></p>",
        escape(text),
        escape(&href(record, Tab::Sources)),
    )
}
