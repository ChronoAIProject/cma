//! The same-origin sign-in and signed-out pages. Neither ever redirects to
//! NyxID by itself: leaving the site takes an explicit click on the link,
//! so a logout on a shared machine stays a logout.
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use super::{document, escape};
use crate::console::redirect::sign_in_href;

/// The sign-in page for a visitor without a session, linking to
/// `/console/login` with `return_to`. `notice` is fixed console text.
pub(crate) fn sign_in(status: StatusCode, return_to: &str, notice: Option<&str>) -> Response {
    let notice = notice.map_or_else(String::new, |text| {
        format!(
            "<p class=\"notice error\" role=\"status\">{}</p>",
            escape(text)
        )
    });
    let body = format!(
        "<section class=\"panel\"><h1>Sign in to the console</h1><p class=\"lead\">See and manage the channels your NyxID account owns, and those of organizations you administer.</p>{notice}<p><a class=\"button\" href=\"{}\">Sign in with NyxID</a></p></section>",
        escape(&sign_in_href(return_to))
    );
    (status, Html(document("Sign in", "", "", &body))).into_response()
}

/// The page after logout, or after a session ended by itself.
pub(crate) fn signed_out(expired: bool) -> Response {
    let lead = if expired {
        "Your console session has ended. Sign in again to continue."
    } else {
        "You signed out of the console."
    };
    let body = format!(
        "<section class=\"panel\"><h1>Signed out</h1><p class=\"lead\">{lead}</p><p>Your NyxID session itself is unchanged; on a shared computer, sign out of NyxID as well.</p><p><a class=\"button\" href=\"{}\">Sign in</a></p></section>",
        escape(&sign_in_href(crate::console::redirect::HOME))
    );
    (StatusCode::OK, Html(document("Signed out", "", "", &body))).into_response()
}
