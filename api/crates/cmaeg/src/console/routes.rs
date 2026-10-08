//! The console's own pages: home, signed-out and logout.
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    Extension,
};
use serde::Deserialize;

use super::{
    authority::Authorized,
    cookies, csrf,
    guard::CurrentSession,
    pages::{self, signin},
    redirect,
    session::EndReason,
    Console,
};
use crate::http::html::escape;

/// `GET /console`. Without a session the extractor renders the sign-in
/// page, never a redirect to NyxID.
pub(crate) async fn home(
    Extension(session): Extension<CurrentSession>,
    _authorized: Authorized,
) -> Response {
    let Some(cell) = session.0 else {
        return signin::sign_in(StatusCode::OK, redirect::HOME, None);
    };
    let (subject, key) = {
        let meta = cell.meta();
        (meta.subject.clone(), meta.csrf_key)
    };
    let logout = pages::post_button(
        redirect::LOGOUT,
        &csrf::token(&key, redirect::LOGOUT, ""),
        "Sign out",
        "secondary",
    );
    let account = format!(
        "<div class=\"account\"><span>Signed in as {}</span>{logout}</div>",
        escape(&mask(&subject))
    );
    let body = "<h1>Console</h1><p class=\"lead\">You are signed in.</p>";
    (
        StatusCode::OK,
        Html(pages::document("Console", &account, body)),
    )
        .into_response()
}

#[derive(Deserialize)]
pub(crate) struct SignedOutQuery {
    reason: Option<String>,
}

/// `GET /console/signed-out`: says the session ended and offers an explicit
/// sign-in link; it never redirects by itself.
pub(crate) async fn signed_out(Query(query): Query<SignedOutQuery>) -> Response {
    signin::signed_out(query.reason.as_deref() == Some("expired"))
}

/// `POST /console/logout`. The guard has checked the origin, the session and
/// the CSRF token. Logout derives no authority and calls NyxID only for the
/// best-effort revocation, so it works while NyxID is down.
pub(crate) async fn logout(
    State(console): State<Console>,
    Extension(session): Extension<CurrentSession>,
) -> Response {
    let Some(cell) = session.0 else {
        return redirect::signed_out(true);
    };
    console.sessions().end(&cell, EndReason::Logout);
    let mut response = redirect::signed_out(false);
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookies::clear(cookies::SESSION));
    response
}

/// A NyxID subject as the console shows it: its last four characters.
pub(crate) fn mask(subject: &str) -> String {
    let tail: String = subject
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}
