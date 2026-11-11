//! Silent sign-in (OIDC `prompt=none`): asks whether the browser already holds
//! a provider session, so a signed-in visitor is not shown the landing.

use std::sync::Arc;

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use serde::Deserialize;

use super::{begin_login_with_intent, LoginAttempt};
use crate::auth::{silent_preference, AuthState};

/// OIDC `prompt`. Only `none` is accepted.
#[derive(Debug, Clone, Copy, Deserialize)]
pub(super) enum LoginPrompt {
    #[serde(rename = "none")]
    Silent,
}

/// Answers a silent entry that must not reach the provider, if any.
pub(super) async fn refuse_entry(
    auth: &AuthState,
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
    silent_preference::suppressed(auth, headers)
        .await
        .then(|| Redirect::to(return_to.unwrap_or("/")).into_response())
}

/// Ends a bound silent attempt the provider declined, or that a CMA sign-out
/// has since superseded. `None` continues as an ordinary callback.
pub(super) async fn declined_callback(
    auth: &Arc<AuthState>,
    attempt: &LoginAttempt,
    error: Option<&str>,
    headers: &HeaderMap,
    binding: &str,
) -> Option<Response> {
    let signed_out = silent_preference::suppressed(auth, headers).await;
    let back = || Redirect::to(attempt.return_to.as_deref().unwrap_or("/")).into_response();
    let response = match error {
        // Signed in to the provider, which only needs this person to approve
        // CMA: continue to its consent screen. Nothing starts work.
        Some("consent_required") if !signed_out && !attempt.consent_requested => {
            begin_login_with_intent(
                auth.clone(),
                attempt.return_to.clone(),
                None,
                None,
                true,
                Some(binding),
            )
            .await
        }
        // No provider session, or the browser signed out of CMA after this
        // attempt began. Nobody asked to sign in, so return them to the page
        // they were on, still signed out.
        Some(_) => back(),
        None if signed_out => back(),
        None => return None,
    };
    Some(response)
}

pub(super) fn fallback(automatic: bool, return_to: Option<&str>, failure: Response) -> Response {
    if automatic {
        Redirect::to(return_to.unwrap_or("/")).into_response()
    } else {
        failure
    }
}
