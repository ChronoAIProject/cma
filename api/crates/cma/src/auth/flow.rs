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
    persistence::{DurableSubjectTokens, LoginAttempt, SecretDigest, SecretValue},
    session_routes::{issued_cookie, logout, read, request_cookie, urlencode},
    signup_options::signup_options,
    token::random_token,
    AuthMode, AuthState, AUTH_COOKIE, AUTH_COOKIE_LIFETIME, SIGNED_OUT_COOKIE,
};

const ATTEMPT_DOMAIN: &str = "oidc-login-attempt";
const BINDING_DOMAIN: &str = "oidc-browser-binding";
const BINDING_COOKIE_PREFIX: &str = "cma_login_";
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

/// OIDC `prompt`. Only `none` is accepted: it asks whether the browser already
/// holds a provider session, so a signed-in visitor is not shown the landing.
#[derive(Debug, Clone, Copy, Deserialize)]
enum LoginPrompt {
    #[serde(rename = "none")]
    Silent,
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
    if silent && request_cookie(&headers, SIGNED_OUT_COOKIE).is_some() {
        // Signed out of CMA: decline without asking the provider.
        return Redirect::to(return_to.as_deref().unwrap_or("/")).into_response();
    }
    if silent && params.screen.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "title": "Invalid sign-in request",
                "detail": "A silent sign-in cannot show a provider screen."
            })),
        )
            .into_response();
    }
    begin_login(auth, return_to, params.screen, silent).await
}

async fn begin_login(
    auth: Arc<AuthState>,
    return_to: Option<String>,
    screen: Option<LoginScreen>,
    silent: bool,
) -> Response {
    begin_login_with_intent(auth, return_to, screen, None, silent).await
}

async fn begin_login_with_intent(
    auth: Arc<AuthState>,
    return_to: Option<String>,
    screen: Option<LoginScreen>,
    actor_link_intent: Option<super::actor_links::ActorLoginIntent>,
    silent: bool,
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
        Err(error) => return provider_failure("identity_discovery_failed", error),
    };
    let verifier = random_token();
    let state = random_token();
    let browser_binding = random_token();
    let cookie_name = binding_cookie_name(&state);
    let attempt = LoginAttempt {
        id: SecretDigest::identify(ATTEMPT_DOMAIN, &state),
        browser_binding: SecretDigest::identify(BINDING_DOMAIN, &browser_binding),
        verifier: SecretValue::new(verifier.clone()),
        return_to,
        actor_link_intent,
        silent,
        expires_at: SystemTime::now() + ATTEMPT_LIFETIME,
    };
    if let Err(error) = auth.store.put_login_attempt(attempt).await {
        tracing::error!(error = %error, "could not persist login attempt");
        return store_failure();
    }
    let redirect_uri = format!("{}/auth/callback", public_url.trim_end_matches('/'));
    let mut authorization_url = match reqwest::Url::parse(&metadata.authorization_endpoint) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => url,
        _ => {
            return provider_failure(
                "identity_discovery_failed",
                "invalid authorization endpoint",
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
    if silent {
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

async fn callback(
    State(auth): State<Arc<AuthState>>,
    Query(params): Query<CallbackParams>,
    headers: HeaderMap,
) -> Response {
    let AuthMode::Provider {
        client_id,
        client_secret,
        public_url,
        ..
    } = &auth.mode
    else {
        return Redirect::to("/").into_response();
    };
    let Some(state) = params.state.as_deref() else {
        return login_failure(StatusCode::BAD_REQUEST, "login_state_missing", None);
    };
    let cookie_name = binding_cookie_name(state);
    let Some(binding) = request_cookie(&headers, &cookie_name) else {
        return login_failure(
            StatusCode::BAD_REQUEST,
            "login_browser_binding_missing",
            Some((&cookie_name, auth.secure_cookies())),
        );
    };
    let attempt = match auth
        .store
        .take_login_attempt(
            &SecretDigest::identify(ATTEMPT_DOMAIN, state),
            &SecretDigest::identify(BINDING_DOMAIN, binding),
        )
        .await
    {
        Ok(Some(attempt)) => attempt,
        Ok(None) => {
            return login_failure(
                StatusCode::BAD_REQUEST,
                "login_state_invalid",
                Some((&cookie_name, auth.secure_cookies())),
            )
        }
        Err(error) => {
            tracing::error!(error = %error, "could not consume login attempt");
            return store_failure();
        }
    };
    if attempt.expires_at <= SystemTime::now() {
        return login_failure(
            StatusCode::BAD_REQUEST,
            "login_state_expired",
            Some((&cookie_name, auth.secure_cookies())),
        );
    }
    if attempt.silent {
        let signed_out = request_cookie(&headers, SIGNED_OUT_COOKIE).is_some();
        let declined = match params.error.as_deref() {
            // Signed in to the provider, which only needs this person to
            // approve CMA: continue to its consent screen. Nothing starts work.
            Some("consent_required") if !signed_out => {
                Some(begin_login(auth.clone(), attempt.return_to.clone(), None, false).await)
            }
            // No provider session, or the browser signed out of CMA after this
            // attempt began. Nobody asked to sign in, so return them to the
            // page they were on, still signed out.
            Some(_) => Some(Redirect::to(attempt.return_to.as_deref().unwrap_or("/")).into_response()),
            None if signed_out => {
                Some(Redirect::to(attempt.return_to.as_deref().unwrap_or("/")).into_response())
            }
            None => None,
        };
        if let Some(mut response) = declined {
            response.headers_mut().append(
                header::SET_COOKIE,
                issued_cookie(&cookie_name, "", auth.secure_cookies(), 0),
            );
            return response;
        }
    }
    if params.error.is_some() {
        return login_failure(
            StatusCode::UNAUTHORIZED,
            "login_cancelled",
            Some((&cookie_name, auth.secure_cookies())),
        );
    }
    let Some(code) = params
        .code
        .as_deref()
        .filter(|code| !code.trim().is_empty())
    else {
        return login_failure(
            StatusCode::BAD_REQUEST,
            "login_code_missing",
            Some((&cookie_name, auth.secure_cookies())),
        );
    };
    let metadata = match auth.metadata().await {
        Ok(metadata) => metadata,
        Err(error) => return provider_failure("identity_discovery_failed", error),
    };
    let redirect_uri = format!("{}/auth/callback", public_url.trim_end_matches('/'));
    let response = match auth
        .http
        .post(&metadata.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code_verifier", attempt.verifier.expose()),
        ])
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => return provider_failure("identity_token_exchange_failed", error.to_string()),
    };
    if !response.status().is_success() {
        return provider_failure(
            "identity_token_exchange_failed",
            format!(
                "identity token endpoint returned HTTP {}",
                response.status().as_u16()
            ),
        );
    }
    let token = match response.json::<TokenResponse>().await {
        Ok(token) if !token.access_token.trim().is_empty() => token,
        _ => return provider_failure("identity_token_exchange_failed", "invalid token response"),
    };
    let lifetime = Duration::from_secs(
        token
            .expires_in
            .unwrap_or(DEFAULT_ACCESS_TOKEN_LIFETIME.as_secs()),
    )
    .min(MAX_ACCESS_TOKEN_LIFETIME);
    let (subject, permission_names, profile) =
        match identity::resolve_subject(&auth, &metadata, &token.access_token).await {
            Ok(identity) => identity,
            Err(error) => return provider_failure("identity_resolution_failed", error),
        };
    let authorization = crate::permissions::capabilities_from_permissions(permission_names);
    if let Err(error) = auth
        .store_sign_in(
            subject.clone(),
            DurableSubjectTokens {
                access_token: SecretValue::new(token.access_token),
                refresh_token: token.refresh_token.map(SecretValue::new),
                expires_at: SystemTime::now() + lifetime,
            },
            authorization,
            profile.clone(),
        )
        .await
    {
        tracing::error!(error = %error, "could not persist signed-in subject");
        return store_failure();
    }
    auth.note_sign_in(&subject, &profile).await;
    let browser_login = match auth.issue_browser_login(subject.clone()).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "could not persist browser login");
            return store_failure();
        }
    };
    if auth
        .retain_actor_login_completion(&attempt, &browser_login, &subject)
        .await
        .is_err()
    {
        return login_failure(
            StatusCode::FORBIDDEN,
            "actor_login_intent_expired",
            Some((&cookie_name, auth.secure_cookies())),
        );
    }
    let mut response = Redirect::to(attempt.return_to.as_deref().unwrap_or("/")).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        issued_cookie(&cookie_name, "", auth.secure_cookies(), 0),
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        issued_cookie(
            AUTH_COOKIE,
            &browser_login,
            auth.secure_cookies(),
            AUTH_COOKIE_LIFETIME.as_secs(),
        ),
    );
    // Only an interactive sign-in reaches here while the marker is set; it
    // is the person choosing to be signed in again.
    if request_cookie(&headers, SIGNED_OUT_COOKIE).is_some() {
        response.headers_mut().append(
            header::SET_COOKIE,
            issued_cookie(SIGNED_OUT_COOKIE, "", auth.secure_cookies(), 0),
        );
    }
    response
}

fn binding_cookie_name(state: &str) -> String {
    let suffix = state.get(..16).unwrap_or(state);
    format!("{BINDING_COOKIE_PREFIX}{suffix}")
}

fn app_return_target(value: &str) -> Result<String, ()> {
    if value == "/" {
        return Ok(value.to_owned());
    }
    if value.len() > 256 || value.contains(['?', '#', '\\', '%']) {
        return Err(());
    }
    let segments = value
        .strip_prefix('/')
        .ok_or(())?
        .split('/')
        .collect::<Vec<_>>();
    if segments.iter().any(|segment| !opaque_path_segment(segment)) {
        return Err(());
    }
    match segments.as_slice() {
        ["agent-manager" | "event-gateway" | "image-manager"] => {}
        ["t", public_id] if opaque_path_segment(public_id) => {}
        ["t", public_id, placement_id]
            if opaque_path_segment(public_id) && opaque_path_segment(placement_id) => {}
        ["triggers", "manage"] => {}
        ["triggers", "manage", trigger_id] if opaque_path_segment(trigger_id) => {}
        ["triggers", "manage", trigger_id, "embed"] if opaque_path_segment(trigger_id) => {}
        ["agents", agent_id] if opaque_path_segment(agent_id) => {}
        ["sandboxes", sandbox_id] if opaque_path_segment(sandbox_id) => {}
        ["bots"] => {}
        ["bots", bot_id] if opaque_path_segment(bot_id) => {}
        ["auth", "bot-father", "link", intent, "complete"] if opaque_path_segment(intent) => {}
        ["profiles"] | ["marketplace"] => {}
        ["profiles" | "marketplace", kind, profile_id]
            if matches!(*kind, "agent" | "workspace" | "sandbox")
                && opaque_path_segment(profile_id) => {}
        _ => return Err(()),
    }
    Ok(value.to_owned())
}

fn opaque_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
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
