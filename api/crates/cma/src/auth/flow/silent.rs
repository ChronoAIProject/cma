//! Silent sign-in (OIDC `prompt=none`): asks whether the browser already holds
//! a provider session, so a signed-in visitor is not shown the landing.

use std::sync::Arc;

use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use serde::Deserialize;

use super::{begin_login, issued_cookie, request_cookie, LoginAttempt};
use crate::auth::{AuthState, SIGNED_OUT_COOKIE};

/// OIDC `prompt`. Only `none` is accepted.
#[derive(Debug, Clone, Copy, Deserialize)]
pub(super) enum LoginPrompt {
    #[serde(rename = "none")]
    Silent,
}

/// Answers a silent entry that must not reach the provider, if any.
pub(super) fn refuse_entry(
    with_screen: bool,
    headers: &HeaderMap,
    return_to: Option<&str>,
) -> Option<Response> {
    if with_screen {
        return Some(
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "title": "Invalid sign-in request",
                    "detail": "A silent sign-in cannot show a provider screen."
                })),
            )
                .into_response(),
        );
    }
    // Signed out of CMA: decline without asking the provider.
    request_cookie(headers, SIGNED_OUT_COOKIE)
        .map(|_| Redirect::to(return_to.unwrap_or("/")).into_response())
}

/// Ends a bound silent attempt the provider declined, or that a CMA sign-out
/// has since superseded. `None` continues as an ordinary callback.
pub(super) async fn declined_callback(
    auth: &Arc<AuthState>,
    attempt: &LoginAttempt,
    error: Option<&str>,
    headers: &HeaderMap,
    binding_cookie: &str,
) -> Option<Response> {
    let signed_out = request_cookie(headers, SIGNED_OUT_COOKIE).is_some();
    let back = || Redirect::to(attempt.return_to.as_deref().unwrap_or("/")).into_response();
    let mut response = match error {
        // Signed in to the provider, which only needs this person to approve
        // CMA: continue to its consent screen. Nothing starts work.
        Some("consent_required") if !signed_out => {
            begin_login(auth.clone(), attempt.return_to.clone(), None, false).await
        }
        // No provider session, or the browser signed out of CMA after this
        // attempt began. Nobody asked to sign in, so return them to the page
        // they were on, still signed out.
        Some(_) => back(),
        None if signed_out => back(),
        None => return None,
    };
    response.headers_mut().append(
        header::SET_COOKIE,
        issued_cookie(binding_cookie, "", auth.secure_cookies(), 0),
    );
    Some(response)
}

/// Only an interactive sign-in completes while the marker is set; it is the
/// person choosing to be signed in again, so the marker is cleared.
pub(super) fn clear_signed_out(response: &mut Response, headers: &HeaderMap, secure: bool) {
    if request_cookie(headers, SIGNED_OUT_COOKIE).is_some() {
        response.headers_mut().append(
            header::SET_COOKIE,
            issued_cookie(SIGNED_OUT_COOKIE, "", secure, 0),
        );
    }
}
