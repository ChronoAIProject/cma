//! The one authority extractor. Every console route that reads or writes,
//! the owner switch included, takes `Authorized`: it derives authority again
//! for this request after the guard has resolved the session (and, for a
//! POST, checked Origin and CSRF). There is no separate mutation extractor
//! (Amendment R6 §R6.4).
use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, Method, StatusCode},
    response::Response,
};

use super::{
    authority::{Authorized, Denied},
    cookies,
    guard::CurrentSession,
    pages::{self, signin},
    redirect,
    session::OwnerSelection,
    Console,
};

/// The owner a POST asks to switch to; set by the guard from the verified
/// form of the owner switch.
#[derive(Clone)]
pub(crate) struct RequestedOwner(pub OwnerSelection);

impl FromRequestParts<Console> for Authorized {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, console: &Console) -> Result<Self, Response> {
        let post = parts.method == Method::POST;
        let Some(cell) = parts
            .extensions
            .get::<CurrentSession>()
            .and_then(|session| session.0.clone())
        else {
            return Err(if post {
                redirect::signed_out(true)
            } else {
                sign_in(parts)
            });
        };
        let authorized = match console.authority().authorize(&cell).await {
            Ok(authorized) => authorized,
            // The flash explains the change, and an organization record now
            // reads as 404 under this Personal authority.
            Err(Denied::Downgraded(personal)) if !post => personal,
            Err(Denied::SignedOut) if !post => return Err(ended(sign_in(parts))),
            Err(denied) => return Err(denied_response(denied)),
        };
        let subject = cell.meta().subject.clone();
        crate::request_audit::bind(
            &subject,
            (authorized.scope() == super::authority::OwnerScope::Organization)
                .then_some(authorized.owner()),
            cmaeg_core::request_audit::Credential::WebSession,
        )
        .await
        .map_err(|_| pages::unavailable())?;
        Ok(authorized)
    }
}

/// What a request whose authority was denied gets; a POST makes no change.
/// Without a session a POST goes to the signed-out page, and a session that
/// just ended also loses its cookie.
pub(crate) fn denied_response(denied: Denied) -> Response {
    match denied {
        Denied::SignedOut => ended(redirect::signed_out(true)),
        // No channel data: the generic page, and the session is kept.
        Denied::Unavailable => pages::unavailable(),
        // The flash explains it on the page the browser lands on.
        Denied::Downgraded(_) | Denied::Refused => redirect::see_other(redirect::HOME),
    }
}

/// Without a session a GET renders the sign-in page, never a redirect to
/// NyxID.
fn sign_in(parts: &Parts) -> Response {
    signin::sign_in(
        StatusCode::OK,
        &redirect::return_to(parts.uri.path_and_query().map(|value| value.as_str())),
        None,
    )
}

fn ended(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookies::clear(cookies::SESSION));
    response
}
