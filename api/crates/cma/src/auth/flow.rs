//! Browser authorization-code flow with PKCE and initiating-browser binding.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::{
    extract::{OriginalUri, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::{
    identity,
    middleware::store_failure,
    persistence::{BrowserLogin, DurableSubjectTokens, LoginAttempt, SecretDigest, SecretValue},
    session_routes::{issued_cookie, logout, read, request_cookie, urlencode},
    signup_options::signup_options,
    token::random_token,
    AuthMode, AuthState, AUTH_COOKIE, AUTH_COOKIE_DOMAIN, AUTH_COOKIE_LIFETIME,
};

mod callback;
use callback::callback;
mod return_target;
mod silent;

use return_target::{app_return_target, opaque_path_segment};
use silent::LoginPrompt;

const ATTEMPT_DOMAIN: &str = "oidc-login-attempt";
const BINDING_DOMAIN: &str = "oidc-browser-binding";
pub(super) const BINDING_COOKIE_PREFIX: &str = "cma_login_";
const ATTEMPT_LIFETIME: Duration = Duration::from_secs(10 * 60);
const DEFAULT_ACCESS_TOKEN_LIFETIME: Duration = Duration::from_secs(15 * 60);
const MAX_ACCESS_TOKEN_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    /// Sent with every provider error; accepted so the error can be handled,
    /// never shown or trusted.
    #[serde(default, rename = "error_description")]
    _error_description: Option<String>,
    #[serde(default, rename = "error_uri")]
    _error_uri: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginParams {
    #[serde(default)]
    return_to: Option<String>,
    #[serde(default)]
    screen: Option<LoginScreen>,
    #[serde(default)]
    prompt: Option<LoginPrompt>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LoginScreen {
    Register,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

pub fn router(auth: Arc<AuthState>) -> Router {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/session", get(read))
        .route("/auth/signup-options", get(signup_options))
        .route("/auth/logout", get(logout))
        .merge(super::actor_links::routes())
        .with_state(auth)
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                if !response.headers().contains_key(header::CACHE_CONTROL) {
                    response
                        .headers_mut()
                        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
                }
                response
            },
        ))
}

/// Reuse the established browser-bound PKCE flow for an opaque actor-link intent.
pub(super) async fn actor_login(auth: Arc<AuthState>, link_id: &str) -> Response {
    if !opaque_path_segment(link_id) || link_id.len() > 100 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let intent = match auth.actor_login_intent(link_id).await {
        Ok(intent) => intent,
        Err(_) => return StatusCode::GONE.into_response(),
    };
    begin_login_with_intent(
        auth,
        Some(format!("/auth/bot-father/link/{link_id}/complete")),
        None,
        intent,
        false,
        None,
    )
    .await
}

async fn login(
    State(auth): State<Arc<AuthState>>,
    Query(params): Query<LoginParams>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    if let Some(redirect) = auth.canonical_login_redirect(&headers, &uri) {
        return redirect;
    }
    let return_to = match params.return_to {
        Some(value) => match app_return_target(&value) {
            Ok(value) => Some(value),
            Err(()) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "title": "Invalid sign-in return target",
                        "detail": "Only a canonical CMA workflow path may be restored."
                    })),
                )
                    .into_response()
            }
        },
        None => None,
    };
    let silent = matches!(params.prompt, Some(LoginPrompt::Silent));
    if silent {
        if let Some(refusal) =
            silent::refuse_entry(params.screen.is_some(), &headers, return_to.as_deref())
        {
            return refusal;
        }
    }
    if silent
        && (headers
            .get("sec-fetch-site")
            .is_some_and(|site| site != "same-origin" && site != "none")
            || (headers.contains_key(header::ORIGIN) && !auth.permits_browser_origin(&headers)))
    {
        return Redirect::to(return_to.as_deref().unwrap_or("/")).into_response();
    }
    begin_login(auth, return_to, params.screen, silent).await
}

async fn begin_login(
    auth: Arc<AuthState>,
    return_to: Option<String>,
    screen: Option<LoginScreen>,
    silent: bool,
) -> Response {
    begin_login_with_intent(auth, return_to, screen, None, silent, None).await
}

async fn begin_login_with_intent(
    auth: Arc<AuthState>,
    return_to: Option<String>,
    screen: Option<LoginScreen>,
    actor_link_intent: Option<super::actor_links::ActorLoginIntent>,
    silent: bool,
    continuation_binding: Option<&str>,
) -> Response {
    let AuthMode::Provider {
        client_id,
        public_url,
        ..
    } = &auth.mode
    else {
        return Redirect::to(return_to.as_deref().unwrap_or("/")).into_response();
    };
    let metadata = match auth.metadata().await {
        Ok(metadata) => metadata,
        Err(error) => {
            return silent::fallback(
                silent,
                return_to.as_deref(),
                provider_failure("identity_discovery_failed", error),
            )
        }
    };
    let verifier = random_token();
    // Presentation marker only; state and binding validation still decide authority.
    let state = if silent {
        format!("s.{}", random_token())
    } else {
        random_token()
    };
    let browser_binding = continuation_binding
        .map(str::to_owned)
        .unwrap_or_else(random_token);
    let expires_at = SystemTime::now() + ATTEMPT_LIFETIME;
    if continuation_binding.is_none() {
        if let Err(error) = auth
            .store
            .reserve_browser_login(
                SecretDigest::identify(AUTH_COOKIE_DOMAIN, &browser_binding),
                expires_at,
            )
            .await
        {
            tracing::error!(error = %error, "could not reserve browser login");
            return silent::fallback(silent, return_to.as_deref(), store_failure());
        }
    }
    let cookie_name = binding_cookie_name(&state);
    let attempt = LoginAttempt {
        id: SecretDigest::identify(ATTEMPT_DOMAIN, &state),
        browser_binding: SecretDigest::identify(BINDING_DOMAIN, &browser_binding),
        verifier: SecretValue::new(verifier.clone()),
        return_to: return_to.clone(),
        actor_link_intent,
        silent,
        consent_requested: continuation_binding.is_some(),
        expires_at,
    };
    if let Err(error) = auth.store.put_login_attempt(attempt).await {
        tracing::error!(error = %error, "could not persist login attempt");
        return silent::fallback(silent, return_to.as_deref(), store_failure());
    }
    let redirect_uri = format!("{}/auth/callback", public_url.trim_end_matches('/'));
    let mut authorization_url = match reqwest::Url::parse(&metadata.authorization_endpoint) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => url,
        _ => {
            return silent::fallback(
                silent,
                return_to.as_deref(),
                provider_failure(
                    "identity_discovery_failed",
                    "invalid authorization endpoint",
                ),
            )
        }
    };
    authorization_url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", &redirect_uri),
        ("scope", "openid profile email roles proxy offline_access"),
        ("state", &state),
        ("code_challenge", &code_challenge(&verifier)),
        ("code_challenge_method", "S256"),
    ]);
    if silent && continuation_binding.is_none() {
        authorization_url
            .query_pairs_mut()
            .append_pair("prompt", "none");
    }
    let destination = match screen {
        Some(LoginScreen::Register) => format!(
            "{}/register?return_to={}",
            auth.nyxid_public_url.trim_end_matches('/'),
            urlencode(authorization_url.as_str()),
        ),
        None => authorization_url.to_string(),
    };
    let mut response = Redirect::to(&destination).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        issued_cookie(
            &cookie_name,
            &browser_binding,
            auth.secure_cookies(),
            ATTEMPT_LIFETIME.as_secs(),
        ),
    );
    response
}

fn binding_cookie_name(state: &str) -> String {
    let suffix = state.get(..16).unwrap_or(state);
    format!("{BINDING_COOKIE_PREFIX}{suffix}")
}

fn code_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn provider_failure(code: &'static str, detail: impl ToString) -> Response {
    tracing::warn!(code, detail = %detail.to_string(), "identity provider operation failed");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "title": "Identity provider unavailable",
            "detail": "The identity provider could not complete this request.",
            "code": code,
        })),
    )
        .into_response()
}

fn login_failure(
    status: StatusCode,
    code: &'static str,
    clear_binding: Option<(&str, bool)>,
) -> Response {
    let mut response = (
        status,
        Json(serde_json::json!({
            "title": "Sign-in could not be completed",
            "detail": "Start a new sign-in attempt.",
            "code": code,
            "login_url": "/auth/login",
        })),
    )
        .into_response();
    if let Some((name, secure)) = clear_binding {
        response
            .headers_mut()
            .append(header::SET_COOKIE, issued_cookie(name, "", secure, 0));
    }
    response
}

#[cfg(test)]
#[path = "flow_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "flow_registration_tests.rs"]
mod registration_tests;

#[cfg(test)]
#[path = "flow_golive_tests.rs"]
mod golive_tests;
