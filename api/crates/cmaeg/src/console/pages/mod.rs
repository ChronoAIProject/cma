//! Server-rendered console pages. They carry no script: every interaction is
//! a link or a same-origin form, and the only resource is the inline
//! stylesheet the CSP admits by hash. Every owner- or NyxID-controlled string
//! goes through `escape`; error pages are generic and never echo upstream
//! bodies.
pub(crate) mod activity;
pub(crate) mod banners;
pub(crate) mod channel;
pub(crate) mod channels;
pub(crate) mod consumers;
pub(crate) mod detach;
pub(crate) mod event;
pub(crate) mod forms;
pub(crate) mod layout;
pub(crate) mod overview;
pub(crate) mod producers;
pub(crate) mod reveal;
pub(crate) mod route;
pub(crate) mod routing;
pub(crate) mod signin;
pub(crate) mod sources;
#[cfg(test)]
mod tab_tests;

use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use super::headers::STYLE;
use crate::http::html::escape;

/// One complete page. `head` (extra head elements), `account` (the header's
/// right side) and `body` are markup the caller built from escaped values.
pub(crate) fn document(title: &str, head: &str, account: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"robots\" content=\"noindex\">{head}<title>{} · CMA Event Gateway</title><style>{STYLE}</style></head><body><header><span class=\"brand\">CMA Event Gateway console</span>{account}</header><main>{body}</main></body></html>",
        escape(title)
    )
}

pub(crate) fn page(status: StatusCode, title: &str, body: &str) -> Response {
    (status, Html(document(title, "", "", body))).into_response()
}

/// The generic page for a console that cannot serve now. It names no cause
/// and carries no channel data: a missing client key, a discovery failure
/// and an unreachable NyxID look the same from outside.
pub(crate) fn unavailable() -> Response {
    page(
        StatusCode::SERVICE_UNAVAILABLE,
        "Console unavailable",
        "<h1>Console unavailable</h1><p class=\"lead\">The console is temporarily unavailable. Please try again in a moment.</p><p><a class=\"button secondary\" href=\"/console\">Try again</a></p>",
    )
}

pub(crate) fn not_found() -> Response {
    page(
        StatusCode::NOT_FOUND,
        "Not found",
        "<h1>Not found</h1><p class=\"lead\">There is nothing at this address.</p><p><a class=\"button secondary\" href=\"/console\">Go to the console</a></p>",
    )
}

/// A refused form: wrong origin, missing or wrong CSRF token, or a body
/// that is not a console form.
pub(crate) fn forbidden() -> Response {
    page(
        StatusCode::FORBIDDEN,
        "Request refused",
        "<h1>Request refused</h1><p class=\"lead\">This request could not be verified. Go back to the console and try again.</p><p><a class=\"button secondary\" href=\"/console\">Go to the console</a></p>",
    )
}

/// A table with `headers` and rows of `<td>` markup the caller escaped.
pub(crate) fn table(headers: &[&str], rows: &[String]) -> String {
    let head: String = headers
        .iter()
        .map(|header| format!("<th>{}</th>", escape(header)))
        .collect();
    let body: String = rows.iter().map(|row| format!("<tr>{row}</tr>")).collect();
    format!("<div class=\"scroll\"><table><thead><tr>{head}</tr></thead><tbody>{body}</tbody></table></div>")
}

/// A definition list of fixed terms and values the caller escaped.
pub(crate) fn facts(items: &[(&str, String)]) -> String {
    let rows: String = items
        .iter()
        .map(|(term, value)| format!("<dt>{}</dt><dd>{value}</dd>", escape(term)))
        .collect();
    format!("<dl class=\"facts\">{rows}</dl>")
}

/// `value` escaped, or a dash when absent.
pub(crate) fn or_dash(value: Option<&str>) -> String {
    value.map_or_else(|| "—".to_owned(), escape)
}

/// A moment as the console shows it: RFC 3339 in UTC, to the second.
pub(crate) fn when(at: time::OffsetDateTime) -> String {
    at.to_offset(time::UtcOffset::UTC)
        .replace_nanosecond(0)
        .unwrap_or(at)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "—".to_owned())
}

/// A hidden CSRF field and a submit button posting to a same-origin path.
pub(crate) fn post_button(action: &str, csrf: &str, label: &str, class: &str) -> String {
    format!(
        "<form class=\"inline\" method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button type=\"submit\" class=\"{}\">{}</button></form>",
        escape(action),
        escape(csrf),
        escape(class),
        escape(label)
    )
}
