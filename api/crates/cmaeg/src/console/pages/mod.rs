//! Server-rendered console pages. They carry no script: every interaction is
//! a link or a same-origin form.
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use crate::http::html::escape;

/// One complete page. `body` is markup the caller built from escaped values.
pub(crate) fn document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"robots\" content=\"noindex\"><title>{} · CMA Event Gateway</title></head><body><header><span class=\"brand\">CMA Event Gateway</span></header><main>{body}</main></body></html>",
        escape(title)
    )
}

pub(crate) fn page(status: StatusCode, title: &str, body: &str) -> Response {
    (status, Html(document(title, body))).into_response()
}

/// The generic page for a console that cannot serve now. It names no cause:
/// a missing client key, a discovery failure and an unreachable NyxID look
/// the same from outside, and none of them carries channel data.
pub(crate) fn unavailable() -> Response {
    page(
        StatusCode::SERVICE_UNAVAILABLE,
        "Console unavailable",
        "<h1>Console unavailable</h1><p>The console is temporarily unavailable. Please try again later.</p>",
    )
}

pub(crate) fn not_found() -> Response {
    page(
        StatusCode::NOT_FOUND,
        "Not found",
        "<h1>Not found</h1><p>There is nothing at this address.</p><p><a href=\"/console\">Go to the console</a></p>",
    )
}
