//! The two authority extractors. `Authorized` is a read's proof and
//! `Fresh<Authorized>` a mutation's: each derives authority again for this
//! request, after the guard has resolved the session (and, for a POST,
//! checked Origin and CSRF).
use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, Method, StatusCode},
    response::Response,
};

use super::{
    authority::{Authorized, Denied, Need},
    cookies,
    guard::CurrentSession,
    pages::{self, signin},
    redirect,
    session::OwnerSelection,
    Console,
};

/// A mutation's authority: verified afresh whatever the session's verdict.
pub struct Fresh<T>(pub T);

/// The owner a POST asks to switch to; set by the guard from the verified
/// form of the owner switch.
#[derive(Clone)]
pub(crate) struct RequestedOwner(pub OwnerSelection);

impl FromRequestParts<Console> for Authorized {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, console: &Console) -> Result<Self, Response> {
        authorize(parts, console, Need::Read).await
    }
}

impl FromRequestParts<Console> for Fresh<Authorized> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, console: &Console) -> Result<Self, Response> {
        let need = parts
            .extensions
            .get::<RequestedOwner>()
            .map_or(Need::Fresh, |requested| Need::Select(requested.0.clone()));
        authorize(parts, console, need).await.map(Fresh)
    }
}

async fn authorize(parts: &Parts, console: &Console, need: Need) -> Result<Authorized, Response> {
    let post = parts.method == Method::POST;
    let Some(cell) = parts
        .extensions
        .get::<CurrentSession>()
        .and_then(|session| session.0.clone())
    else {
        return Err(signed_out(parts, post, false));
    };
    match console.authority().authorize(&cell, need).await {
        Ok(authorized) => Ok(authorized),
        Err(Denied::SignedOut) => Err(signed_out(parts, post, true)),
        // No channel data: the generic page, and the session is kept.
        Err(Denied::Unavailable) => Err(pages::unavailable()),
        // The flash explains it on the page the browser lands on.
        Err(Denied::Downgraded | Denied::Refused) => Err(redirect::see_other(redirect::HOME)),
    }
}

/// Without a session a GET renders the sign-in page and a POST goes to the
/// signed-out page; a session that just ended also loses its cookie.
fn signed_out(parts: &Parts, post: bool, ended: bool) -> Response {
    let mut response = if post {
        redirect::signed_out(true)
    } else {
        signin::sign_in(
            StatusCode::OK,
            &redirect::return_to(parts.uri.path_and_query().map(|value| value.as_str())),
            None,
        )
    };
    if ended {
        response
            .headers_mut()
            .insert(header::SET_COOKIE, cookies::clear(cookies::SESSION));
    }
    response
}
