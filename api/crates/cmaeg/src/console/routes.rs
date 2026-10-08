//! The console's own pages: home, the owner switch, signed-out and logout.
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::header,
    response::Response,
    Extension,
};
use serde::Deserialize;

use super::{
    authority::{Authorized, Denied},
    cookies, csrf,
    extract::{denied_response, RequestedOwner},
    guard::CurrentSession,
    pages::{layout, signin},
    redirect::{self, HOME},
    session::{EndReason, Flash, OwnerSelection},
    subject_hash_prefix, Console,
};

#[derive(Deserialize)]
pub(crate) struct HomeQuery {
    page: Option<usize>,
    q: Option<String>,
}

/// `GET /console`: the signed-in layout. Without a session the extractor
/// renders the sign-in page, never a redirect to NyxID.
pub(crate) async fn home(
    State(console): State<Console>,
    Extension(session): Extension<CurrentSession>,
    authorized: Authorized,
    query: Result<Query<HomeQuery>, QueryRejection>,
) -> Response {
    let Some(cell) = session.0 else {
        return redirect::see_other(HOME);
    };
    let orgs = console.org_choices(&cell).await;
    let (subject, selected, key, flash) = {
        let mut meta = cell.meta();
        (
            meta.subject.clone(),
            meta.selected.clone(),
            meta.csrf_key,
            meta.flash.take(),
        )
    };
    let (page, filter) = query.map_or((0, None), |Query(query)| (query.page.unwrap_or(0), query.q));
    let channels = console
        .directory()
        .channels_for(&authorized, page, filter.as_deref());
    layout::render(&layout::LayoutView {
        subject: &mask(&subject),
        selected: &selected,
        orgs: &orgs,
        owner_csrf: &csrf::token(&key, redirect::OWNER, ""),
        logout_csrf: &csrf::token(&key, redirect::LOGOUT, ""),
        flash: flash.as_ref(),
        channels: &channels,
    })
}

/// `POST /console/owner`: the switch takes the same `Authorized` extractor as
/// every other route, so the current selection is verified first; then the
/// requested owner is verified exactly as `X-CMAEG-Owner` is, through REST's
/// cache, and only then stored. A refused choice leaves the selection
/// unchanged with a notice.
pub(crate) async fn switch_owner(
    State(console): State<Console>,
    Extension(session): Extension<CurrentSession>,
    requested: Option<Extension<RequestedOwner>>,
    _current: Authorized,
) -> Response {
    let Some(cell) = session.0 else {
        return redirect::signed_out(true);
    };
    let subject_hash = subject_hash_prefix(&cell.meta().subject);
    let (outcome, owner_kind) = match requested {
        Some(Extension(RequestedOwner(requested))) => {
            let owner_kind = match requested {
                OwnerSelection::Personal => "personal",
                OwnerSelection::Organization(_) => "organization",
            };
            match console.authority().select(&cell, requested).await {
                Ok(_) => ("ok", owner_kind),
                Err(Denied::Refused) => ("refused", owner_kind),
                Err(denied) => return denied_response(denied),
            }
        }
        None => {
            cell.meta().flash = Some(Flash::OwnerRefused);
            ("refused", "unknown")
        }
    };
    tracing::info!(
        action = "owner",
        record_hash = "-",
        outcome,
        subject_hash_prefix = %subject_hash,
        owner_kind,
        "console_action"
    );
    redirect::see_other(HOME)
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
