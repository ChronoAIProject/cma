//! The authorization-code flow: redirect out, exchange on the way back.
//!
//! Split from the module that owns auth state so each file stays about one
//! thing: this one is the browser-facing dance, `super` is what the rest of
//! the service asks "who is calling".

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use layer2_core::{LaunchId, PublicTriggerId, SubjectId};
use serde::Deserialize;

use super::permissions::{introspect_permissions, permissions_claim};
use super::persistence::{DurableSubjectTokens, LoginAttempt, SecretDigest, SecretValue};
use super::session_routes::{logout, session, urlencode};
use super::{code_challenge, random_token, AuthMode, AuthState, ProviderMetadata, SESSION_COOKIE};

const LOGIN_ATTEMPT_DOMAIN: &str = "login-attempt";
const LOGIN_BROWSER_BINDING_DOMAIN: &str = "login-browser-binding";
const LOGIN_BINDING_COOKIE_PREFIX: &str = "cma_login_binding_";
const LOGIN_ATTEMPT_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// Query parameters returned by the provider on the redirect back.
#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginParams {
    #[serde(default)]
    return_to: Option<String>,
}

/// Routes for the login flow.
pub fn router(auth: Arc<AuthState>) -> Router {
    Router::new()
        .route("/auth/login", get(login_route))
        .route("/auth/callback", get(callback))
        .route("/auth/session", get(session))
        .route("/auth/logout", get(logout))
        .with_state(auth)
}

#[cfg(test)]
async fn login(State(auth): State<Arc<AuthState>>) -> Response {
    begin_login(auth, None).await
}

async fn login_route(
    State(auth): State<Arc<AuthState>>,
    Query(params): Query<LoginParams>,
) -> Response {
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
    begin_login(auth, return_to).await
}

async fn begin_login(auth: Arc<AuthState>, return_to: Option<String>) -> Response {
    match &auth.mode {
        AuthMode::Trusted { .. } => {
            Redirect::to(return_to.as_deref().unwrap_or("/")).into_response()
        }
        AuthMode::Provider {
            client_id,
            public_url,
            ..
        } => match auth.metadata().await {
            Ok(meta) => {
                let redirect_uri = format!("{}/auth/callback", public_url.trim_end_matches('/'));
                // PKCE is mandatory at this provider. The verifier stays here;
                // only its digest travels through the browser.
                let verifier = random_token();
                let state = random_token();
                let browser_binding = random_token();
                let challenge = code_challenge(&verifier);
                if let Err(error) = auth
                    .store
                    .put_login_attempt(LoginAttempt {
                        id: SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, &state),
                        browser_binding: SecretDigest::identify(
                            LOGIN_BROWSER_BINDING_DOMAIN,
                            &browser_binding,
                        ),
                        verifier: SecretValue::new(verifier),
                        return_to,
                        expires_at: SystemTime::now() + LOGIN_ATTEMPT_LIFETIME,
                    })
                    .await
                {
                    tracing::error!(error = %error, "could not store a login attempt");
                    return layer1_public::auth::StoreUnavailable.into_response();
                }
                let url = format!(
                    "{}?response_type=code&client_id={}&redirect_uri={}&scope=openid%20profile%20email%20roles%20proxy%20offline_access\
&state={}&code_challenge={}&code_challenge_method=S256",
                    meta.authorization_endpoint,
                    urlencode(client_id),
                    urlencode(&redirect_uri),
                    urlencode(&state),
                    urlencode(&challenge),
                );
                let mut response = Redirect::to(&url).into_response();
                append_set_cookie(
                    &mut response,
                    issued_login_binding_cookie(
                        &login_binding_cookie_name(&state),
                        &browser_binding,
                        auth.secure_cookies(),
                    ),
                );
                response
            }
            Err(e) => {
                tracing::error!(error = %e, "cannot start login: discovery failed");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "title": "Identity provider unavailable",
                        "detail": e,
                    })),
                )
                    .into_response()
            }
        },
    }
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

    // State in the callback URL is insufficient: copied callback URLs must not
    // sign another browser into the provider subject that authorized them.
    let Some(state) = params.state.as_ref() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "title": "Missing state parameter",
                "code": "login_state_missing",
                "login_url": "/auth/login",
            })),
        )
            .into_response();
    };
    let binding_cookie_name = login_binding_cookie_name(state);
    let Some(browser_binding) = request_cookie(&headers, &binding_cookie_name) else {
        tracing::warn!("callback did not carry its initiating-browser binding");
        return invalid_login_attempt_response(&binding_cookie_name, auth.secure_cookies());
    };
    let id = SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, state);
    let binding_id = SecretDigest::identify(LOGIN_BROWSER_BINDING_DOMAIN, browser_binding);
    let attempt = match auth.store.take_login_attempt(&id, &binding_id).await {
        Ok(Some(attempt)) => attempt,
        Ok(None) => {
            tracing::warn!("callback carried an unknown, replayed, or browser-mismatched state");
            return invalid_login_attempt_response(&binding_cookie_name, auth.secure_cookies());
        }
        Err(error) => {
            tracing::error!(error = %error, "could not consume a login attempt");
            return layer1_public::auth::StoreUnavailable.into_response();
        }
    };
    if attempt.expires_at <= SystemTime::now() {
        tracing::warn!("callback carried an expired state");
        return finish_login_failure(
            attempt.return_to.as_deref(),
            "expired",
            &binding_cookie_name,
            auth.secure_cookies(),
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "title": "Login attempt expired",
                    "code": "login_state_expired",
                    "login_url": "/auth/login",
                })),
            )
                .into_response(),
        );
    }
    let LoginAttempt {
        verifier,
        return_to,
        ..
    } = attempt;

    // Every callback attempt consumes its state, including a provider refusal
    // or malformed response. Otherwise the same authorization attempt remains
    // replayable until TTL cleanup merely because this branch needed no code.
    if let Some(err) = params.error {
        tracing::warn!(error = %err, "provider returned an error on callback");
        let reason = if err == "access_denied" {
            "cancelled"
        } else {
            "failed"
        };
        return finish_login_failure(
            return_to.as_deref(),
            reason,
            &binding_cookie_name,
            auth.secure_cookies(),
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "title": "Login failed", "detail": err })),
            )
                .into_response(),
        );
    }
    let Some(code) = params.code else {
        return finish_login_failure(
            return_to.as_deref(),
            "failed",
            &binding_cookie_name,
            auth.secure_cookies(),
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "title": "Missing authorization code" })),
            )
                .into_response(),
        );
    };

    let meta = match auth.metadata().await {
        Ok(m) => m,
        Err(e) => {
            return finish_login_failure(
                return_to.as_deref(),
                "failed",
                &binding_cookie_name,
                auth.secure_cookies(),
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "title": "Discovery failed", "detail": e })),
                )
                    .into_response(),
            );
        }
    };

    let redirect_uri = format!("{}/auth/callback", public_url.trim_end_matches('/'));
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("code_verifier", verifier.expose()),
    ];

    let resp = match auth
        .http
        .post(&meta.token_endpoint)
        .form(&form)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            // The error is logged without the form, which carries the secret.
            tracing::error!(error = %e, "token exchange request failed");
            return finish_login_failure(
                return_to.as_deref(),
                "failed",
                &binding_cookie_name,
                auth.secure_cookies(),
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "title": "Token exchange failed" })),
                )
                    .into_response(),
            );
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "token endpoint rejected the exchange");
        return finish_login_failure(
            return_to.as_deref(),
            "failed",
            &binding_cookie_name,
            auth.secure_cookies(),
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "title": "Token exchange rejected" })),
            )
                .into_response(),
        );
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        #[serde(default)]
        expires_in: Option<u64>,
    }
    let token: TokenResponse = match resp.json().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "token response was not usable");
            return finish_login_failure(
                return_to.as_deref(),
                "failed",
                &binding_cookie_name,
                auth.secure_cookies(),
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "title": "Token response not usable" })),
                )
                    .into_response(),
            );
        }
    };

    let (subject, permissions, profile) =
        match resolve_subject(&auth, &meta, &token.access_token).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "cannot resolve subject");
                return finish_login_failure(
                    return_to.as_deref(),
                    "failed",
                    &binding_cookie_name,
                    auth.secure_cookies(),
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "title": "Cannot resolve subject",
                            "detail": e,
                        })),
                    )
                        .into_response(),
                );
            }
        };

    // Held so the runtime can act as this subject, and so the access token can
    // be renewed before it expires under a running session.
    let has_refresh_token = token.refresh_token.is_some();
    let authorization = crate::permissions::capabilities_from_permissions(&permissions);
    if let Err(error) = auth
        .store_sign_in(
            &subject,
            DurableSubjectTokens {
                access_token: Some(SecretValue::new(token.access_token)),
                refresh_token: token.refresh_token.map(SecretValue::new),
                expires_at: SystemTime::now()
                    + Duration::from_secs(token.expires_in.unwrap_or(3600)),
            },
            authorization.clone(),
            profile.clone(),
        )
        .await
    {
        tracing::error!(%subject, error = %error, "could not persist completed sign-in");
        return finish_login_failure(
            return_to.as_deref(),
            "failed",
            &binding_cookie_name,
            auth.secure_cookies(),
            layer1_public::auth::StoreUnavailable.into_response(),
        );
    }
    if !has_refresh_token {
        // Without one, a long session ends when the access token does. Say so
        // once, at the point the decision is visible.
        tracing::warn!(
            %subject,
            "provider issued no refresh token; delegated access will expire and cannot be renewed"
        );
    }

    // Capabilities are resolved initially at sign-in from the permissions the
    // provider reported. The recurring authorization sweep replaces this
    // complete answer while the delegation remains stored. A caller with none
    // authenticates successfully and is then refused every operation, which is
    // the correct outcome for someone who has a platform account but no access
    // here.
    // Written down before the sign-in completes, because this is the only
    // moment the provider's account details are in hand: a person not recorded
    // here is a person the service can never name afterwards.
    auth.note_sign_in(&subject, &profile).await;
    if authorization.capabilities().count() == 0 {
        tracing::warn!(%subject, "signed in with no capabilities for this service");
    }

    let session = match auth.issue_session(subject.clone()).await {
        Ok(session) => session,
        Err(error) => {
            tracing::error!(%subject, error = %error, "could not persist browser session");
            return finish_login_failure(
                return_to.as_deref(),
                "failed",
                &binding_cookie_name,
                auth.secure_cookies(),
                layer1_public::auth::StoreUnavailable.into_response(),
            );
        }
    };
    // Host-only, HttpOnly, SameSite=Lax: the cookie is never readable from
    // script, and never sent on a cross-site request that could act as the user.
    // Production HTTPS also marks it Secure; explicit HTTP is retained for
    // local development where a Secure cookie would never be returned.
    let cookie = issued_session_cookie(&session, auth.secure_cookies());
    let location = return_to.unwrap_or_else(|| "/".to_string());
    let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response();
    append_set_cookie(&mut response, cookie);
    append_set_cookie(
        &mut response,
        expired_login_binding_cookie(&binding_cookie_name, auth.secure_cookies()),
    );
    response
}

fn app_return_target(value: &str) -> Result<String, ()> {
    if value.len() > 128 || value.contains(['?', '#', '\\']) {
        return Err(());
    }

    let segments: Vec<_> = value.split('/').collect();
    match segments.as_slice() {
        ["", "triggers", public_id] => {
            PublicTriggerId::parse((*public_id).to_string()).map_err(|_| ())?;
        }
        ["", "launches", launch_id]
        | ["", "launches", launch_id, "recover"]
        | ["", "launches", launch_id, "contribute"] => {
            LaunchId::parse((*launch_id).to_string()).map_err(|_| ())?;
        }
        ["", "sessions", session_id] if opaque_path_segment(session_id) => {}
        _ => return Err(()),
    }
    Ok(value.to_string())
}

fn opaque_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
}

fn issued_session_cookie(session: &str, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={session}; Path=/; HttpOnly{secure}; SameSite=Lax; Max-Age=86400")
}

fn login_binding_cookie_name(state: &str) -> String {
    let digest = SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, state);
    let suffix: String = digest
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{LOGIN_BINDING_COOKIE_PREFIX}{suffix}")
}

fn issued_login_binding_cookie(name: &str, value: &str, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!(
        "{name}={value}; Path=/auth/callback; HttpOnly{secure}; SameSite=Lax; Max-Age={}",
        LOGIN_ATTEMPT_LIFETIME.as_secs()
    )
}

fn expired_login_binding_cookie(name: &str, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{name}=; Path=/auth/callback; HttpOnly{secure}; SameSite=Lax; Max-Age=0")
}

fn request_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(candidate, value)| (candidate == name && !value.is_empty()).then_some(value))
}

fn append_set_cookie(response: &mut Response, cookie: String) {
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("CMA cookie values use HTTP-safe random tokens"),
    );
}

fn invalid_login_attempt_response(binding_cookie_name: &str, secure: bool) -> Response {
    let mut response = (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "title": "Unrecognised login attempt",
            "code": "login_state_invalid",
            "login_url": "/auth/login",
        })),
    )
        .into_response();
    append_set_cookie(
        &mut response,
        expired_login_binding_cookie(binding_cookie_name, secure),
    );
    response
}

fn finish_login_failure(
    return_to: Option<&str>,
    reason: &str,
    binding_cookie_name: &str,
    secure: bool,
    fallback: Response,
) -> Response {
    let mut response = match return_to {
        Some(return_to) => (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, format!("{return_to}?auth_error={reason}"))],
        )
            .into_response(),
        None => fallback,
    };
    append_set_cookie(
        &mut response,
        expired_login_binding_cookie(binding_cookie_name, secure),
    );
    response
}

pub(crate) async fn resolve_subject(
    auth: &AuthState,
    meta: &ProviderMetadata,
    access_token: &str,
) -> Result<(SubjectId, Vec<String>, crate::auth::profile::Profile), String> {
    let Some(endpoint) = meta.userinfo_endpoint.as_ref() else {
        return Err("provider advertises no userinfo endpoint".into());
    };
    let resp = auth
        .http
        .get(endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("userinfo request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("userinfo returned {}", resp.status()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("userinfo body not usable: {e}"))?;
    let subject = body
        .get("sub")
        .and_then(|v| v.as_str())
        .map(|s| SubjectId(s.to_string()))
        .ok_or_else(|| "userinfo carried no subject claim".to_string())?;

    // Asked of introspection first, because userinfo only carries these when
    // the token was granted a scope asking for them. Tokens this service did
    // not mint are the ordinary case on the bearer path and carry no such
    // scope, so reading userinfo alone leaves every one of them with nothing
    // and denies a caller the provider considers fully entitled.
    let permissions = match introspect_permissions(auth, meta, access_token).await {
        Ok(Some(found)) => found,
        Ok(None) => permissions_claim(&body),
        Err(e) => {
            // Not fatal on its own: the token already identified a subject, so
            // the caller is who they say they are and only their grants are
            // unknown. Falling back to userinfo keeps a provider that offers no
            // introspection working exactly as before.
            tracing::warn!(error = %e, "could not ask the provider about this token's grants");
            permissions_claim(&body)
        }
    };
    // Capture display material at sign-in. Recurring authorization checks also
    // call userinfo, but deliberately update only the security-sensitive grant.
    let profile = crate::auth::profile::Profile::from_userinfo(&body);
    Ok((subject, permissions, profile))
}

#[cfg(test)]
mod tests {
    use axum::{
        extract::Form,
        http::HeaderMap,
        routing::{get, post},
        Router,
    };
    use std::collections::HashMap;

    use super::*;
    use crate::auth::persistence::{AuthStore, MemoryAuthStore};

    /// Read a handler's response body as JSON.
    async fn body_of(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a body");
        serde_json::from_slice(&bytes).expect("json")
    }

    fn provider_auth() -> Arc<AuthState> {
        AuthState::new(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "not-a-real-value".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
        )
    }

    fn callback_headers(state: &str, browser_binding: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{}={browser_binding}", login_binding_cookie_name(state))
                .parse()
                .expect("valid callback cookie"),
        );
        headers
    }

    #[test]
    fn a_new_login_issues_only_the_canonical_cookie_name() {
        assert_eq!(
            issued_session_cookie("secret", false),
            "cma_session=secret; Path=/; HttpOnly; SameSite=Lax; Max-Age=86400"
        );
    }

    #[test]
    fn an_https_login_marks_the_session_cookie_secure() {
        assert_eq!(
            issued_session_cookie("secret", true),
            "cma_session=secret; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=86400"
        );
    }

    #[test]
    fn login_binding_cookies_are_host_only_callback_scoped_and_short_lived() {
        let name = login_binding_cookie_name("opaque-state");
        assert!(name.starts_with(LOGIN_BINDING_COOKIE_PREFIX));
        assert!(!name.contains("opaque-state"));
        assert_eq!(
            issued_login_binding_cookie(&name, "browser-secret", true),
            format!(
                "{name}=browser-secret; Path=/auth/callback; HttpOnly; Secure; SameSite=Lax; Max-Age=600"
            )
        );
        assert_eq!(
            expired_login_binding_cookie(&name, true),
            format!("{name}=; Path=/auth/callback; HttpOnly; Secure; SameSite=Lax; Max-Age=0")
        );
    }

    #[tokio::test]
    async fn the_identity_answer_says_what_this_caller_may_do() {
        // Without this a client has to guess, and the only things it could
        // guess from are a role name it should never see or a refusal it would
        // have to provoke.
        let auth = provider_auth();
        let subject = SubjectId("subject-a".into());
        auth.store_sign_in(
            &subject,
            DurableSubjectTokens {
                access_token: Some(SecretValue::new("access-a".into())),
                refresh_token: None,
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            },
            layer2_core::Authorization::from_capabilities([
                layer2_core::Capability::ReadAllSessions,
                layer2_core::Capability::ControlAnySession,
            ]),
            crate::auth::profile::Profile::default(),
        )
        .await
        .expect("sign-in persists");
        let token = auth.issue_session(subject).await.expect("session persists");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}={token}")
                .parse()
                .expect("valid header"),
        );

        let json = body_of(session(State(auth), headers).await).await;
        assert_eq!(json["authenticated"], true);
        assert_eq!(json["subject"], "subject-a");
        let capabilities = json["capabilities"].as_array().expect("an array");
        assert!(capabilities.iter().any(|c| c == "read_all_sessions"));
        assert!(capabilities.iter().any(|c| c == "control_any_session"));
        assert_eq!(capabilities.len(), 2, "got {json}");
    }

    #[tokio::test]
    async fn a_caller_granted_nothing_is_told_they_may_do_nothing() {
        // An empty list rather than an absent field: a client that reads a
        // missing key as "unknown" and one that reads it as "nothing" would
        // behave differently, and only one of them fails closed.
        let auth = provider_auth();
        let token = auth
            .issue_session(SubjectId("stranger".into()))
            .await
            .expect("session persists");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}={token}")
                .parse()
                .expect("valid header"),
        );

        let json = body_of(session(State(auth), headers).await).await;
        assert_eq!(json["authenticated"], true);
        assert!(
            json["capabilities"]
                .as_array()
                .expect("an array")
                .is_empty(),
            "got {json}"
        );
    }

    #[tokio::test]
    async fn an_unauthenticated_caller_is_told_nothing_at_all() {
        let json = body_of(session(State(provider_auth()), HeaderMap::new()).await).await;
        assert_eq!(json["authenticated"], false);
        assert!(
            json.get("capabilities").is_none(),
            "a caller with no identity has no entitlement to report: {json}"
        );
    }

    #[tokio::test]
    async fn login_attempt_store_failure_is_a_typed_503() {
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "not-a-real-value".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
            store.clone(),
        );
        *auth.metadata.write().await = Some(ProviderMetadata {
            authorization_endpoint: "http://provider.invalid/authorize".into(),
            token_endpoint: "http://provider.invalid/token".into(),
            userinfo_endpoint: None,
            introspection_endpoint: None,
        });
        store.set_unavailable(true);

        let response = login(State(auth)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body_of(response).await["code"], "store_failed");
    }

    #[tokio::test]
    async fn callback_consume_store_failure_is_a_typed_503() {
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "not-a-real-value".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
            store.clone(),
        );
        store.set_unavailable(true);

        let response = callback(
            State(auth),
            Query(CallbackParams {
                code: Some("authorization-code".into()),
                state: Some("opaque-state".into()),
                error: None,
            }),
            callback_headers("opaque-state", "browser-binding"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body_of(response).await["code"], "store_failed");
    }

    #[tokio::test]
    async fn expired_callback_state_is_consumed_but_never_accepted() {
        let store = Arc::new(MemoryAuthStore::default());
        let state = "opaque-state";
        let browser_binding = "expired-browser-binding";
        store
            .put_login_attempt(LoginAttempt {
                id: SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, state),
                browser_binding: SecretDigest::identify(
                    LOGIN_BROWSER_BINDING_DOMAIN,
                    browser_binding,
                ),
                verifier: SecretValue::new("verifier".into()),
                return_to: Some("/triggers/123e4567-e89b-12d3-a456-426614174000".into()),
                expires_at: SystemTime::now() - Duration::from_secs(1),
            })
            .await
            .expect("fixture persists");
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "not-a-real-value".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
            store.clone(),
        );

        let response = callback(
            State(auth),
            Query(CallbackParams {
                code: Some("authorization-code".into()),
                state: Some(state.into()),
                error: None,
            }),
            callback_headers(state, browser_binding),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/triggers/123e4567-e89b-12d3-a456-426614174000?auth_error=expired"
        );
        assert!(store
            .take_login_attempt(
                &SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, state),
                &SecretDigest::identify(LOGIN_BROWSER_BINDING_DOMAIN, browser_binding),
            )
            .await
            .expect("store works")
            .is_none());
    }

    #[tokio::test]
    async fn a_provider_error_callback_still_consumes_its_state() {
        let store = Arc::new(MemoryAuthStore::default());
        let state = "refused-state";
        let browser_binding = "refused-browser-binding";
        let id = SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, state);
        store
            .put_login_attempt(LoginAttempt {
                id: id.clone(),
                browser_binding: SecretDigest::identify(
                    LOGIN_BROWSER_BINDING_DOMAIN,
                    browser_binding,
                ),
                verifier: SecretValue::new("verifier".into()),
                return_to: Some("/triggers/123e4567-e89b-12d3-a456-426614174000".into()),
                expires_at: SystemTime::now() + Duration::from_secs(60),
            })
            .await
            .expect("fixture persists");
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "not-a-real-value".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
            store.clone(),
        );

        let response = callback(
            State(auth),
            Query(CallbackParams {
                code: None,
                state: Some(state.into()),
                error: Some("access_denied".into()),
            }),
            callback_headers(state, browser_binding),
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/triggers/123e4567-e89b-12d3-a456-426614174000?auth_error=cancelled"
        );
        assert!(store
            .take_login_attempt(
                &id,
                &SecretDigest::identify(LOGIN_BROWSER_BINDING_DOMAIN, browser_binding),
            )
            .await
            .expect("store works")
            .is_none());
    }

    #[tokio::test]
    async fn a_callback_from_another_browser_is_rejected_without_consuming_state() {
        let store = Arc::new(MemoryAuthStore::default());
        let state = "browser-bound-state";
        let browser_binding = "initiating-browser";
        let id = SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, state);
        let binding_id = SecretDigest::identify(LOGIN_BROWSER_BINDING_DOMAIN, browser_binding);
        store
            .put_login_attempt(LoginAttempt {
                id: id.clone(),
                browser_binding: binding_id.clone(),
                verifier: SecretValue::new("verifier".into()),
                return_to: Some("/triggers/123e4567-e89b-12d3-a456-426614174000".into()),
                expires_at: SystemTime::now() + Duration::from_secs(60),
            })
            .await
            .expect("fixture persists");
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "not-a-real-value".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
            store.clone(),
        );

        let response = callback(
            State(auth),
            Query(CallbackParams {
                code: None,
                state: Some(state.into()),
                error: Some("access_denied".into()),
            }),
            callback_headers(state, "different-browser"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_of(response).await["code"], "login_state_invalid");
        assert!(store
            .take_login_attempt(&id, &binding_id)
            .await
            .expect("store works")
            .is_some());
    }

    #[tokio::test]
    async fn parallel_login_tabs_keep_independent_browser_bindings() {
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "not-a-real-value".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
            store,
        );
        *auth.metadata.write().await = Some(ProviderMetadata {
            authorization_endpoint: "http://provider.invalid/authorize".into(),
            token_endpoint: "http://provider.invalid/token".into(),
            userinfo_endpoint: None,
            introspection_endpoint: None,
        });
        let targets = [
            "/triggers/123e4567-e89b-12d3-a456-426614174000",
            "/triggers/223e4567-e89b-12d3-a456-426614174000",
        ];
        let mut states = Vec::new();
        let mut cookies = Vec::new();
        for target in targets {
            let started = begin_login(auth.clone(), Some(target.into())).await;
            let redirect = reqwest::Url::parse(
                started.headers()[header::LOCATION]
                    .to_str()
                    .expect("authorization redirect"),
            )
            .expect("redirect URL");
            states.push(
                redirect
                    .query_pairs()
                    .find_map(|(name, value)| (name == "state").then(|| value.into_owned()))
                    .expect("state"),
            );
            cookies.push(
                started
                    .headers()
                    .get_all(header::SET_COOKIE)
                    .iter()
                    .filter_map(|value| value.to_str().ok())
                    .find(|value| value.starts_with(LOGIN_BINDING_COOKIE_PREFIX))
                    .and_then(|value| value.split(';').next())
                    .expect("binding cookie")
                    .to_string(),
            );
        }
        assert_ne!(states[0], states[1]);
        assert_ne!(cookies[0], cookies[1]);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            cookies
                .join("; ")
                .parse()
                .expect("combined browser cookies"),
        );

        for (state, target) in states.into_iter().zip(targets) {
            let response = callback(
                State(auth.clone()),
                Query(CallbackParams {
                    code: None,
                    state: Some(state),
                    error: Some("access_denied".into()),
                }),
                headers.clone(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(
                response.headers()[header::LOCATION],
                format!("{target}?auth_error=cancelled")
            );
        }
    }

    async fn exchange_code(Form(form): Form<HashMap<String, String>>) -> Json<serde_json::Value> {
        assert_eq!(
            form.get("grant_type").map(String::as_str),
            Some("authorization_code")
        );
        assert_eq!(form.get("code").map(String::as_str), Some("code-a"));
        assert!(form
            .get("code_verifier")
            .is_some_and(|value| !value.is_empty()));
        Json(serde_json::json!({
            "access_token": "access-a",
            "refresh_token": "refresh-a",
            "expires_in": 3600,
        }))
    }

    async fn userinfo(headers: HeaderMap) -> Json<serde_json::Value> {
        assert_eq!(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer access-a")
        );
        Json(serde_json::json!({
            "sub": "subject-a",
            "name": "Subject A",
            "email": "subject-a@example.test",
            "permissions": ["cma:session:create"],
        }))
    }

    #[tokio::test]
    async fn login_on_one_instance_completes_on_another_and_survives_restart() {
        let app = Router::new()
            .route("/token", post(exchange_code))
            .route("/userinfo", get(userinfo));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind identity provider");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve identity provider");
        });

        let store = Arc::new(MemoryAuthStore::default());
        let mode = AuthMode::Provider {
            base_url: base.clone(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            public_url: "http://service.invalid".into(),
        };
        let first = AuthState::with_store(mode.clone(), None, store.clone());
        let second = AuthState::with_store(mode.clone(), None, store.clone());
        let metadata = ProviderMetadata {
            authorization_endpoint: "http://browser.invalid/authorize".into(),
            token_endpoint: format!("{base}/token"),
            userinfo_endpoint: Some(format!("{base}/userinfo")),
            introspection_endpoint: None,
        };
        *first.metadata.write().await = Some(metadata.clone());
        *second.metadata.write().await = Some(metadata);

        let return_to = "/triggers/123e4567-e89b-12d3-a456-426614174000";
        let started = begin_login(first, Some(return_to.to_string())).await;
        assert_eq!(started.status(), StatusCode::SEE_OTHER);
        let location = started
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("authorization redirect")
            .to_string();
        let binding_cookie = started
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find(|value| value.starts_with(LOGIN_BINDING_COOKIE_PREFIX))
            .and_then(|value| value.split(';').next())
            .expect("browser-binding cookie")
            .to_string();
        let redirect = reqwest::Url::parse(&location).expect("redirect is a URL");
        let state = redirect
            .query_pairs()
            .find_map(|(name, value)| (name == "state").then(|| value.into_owned()))
            .expect("redirect carries state");

        let completed = callback(
            State(second),
            Query(CallbackParams {
                code: Some("code-a".into()),
                state: Some(state),
                error: None,
            }),
            {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::COOKIE,
                    binding_cookie.parse().expect("valid callback cookie"),
                );
                headers
            },
        )
        .await;
        assert_eq!(completed.status(), StatusCode::SEE_OTHER);
        assert_eq!(completed.headers()[header::LOCATION], return_to);
        let set_cookie = completed
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("session cookie");
        let raw_cookie = set_cookie
            .split(';')
            .next()
            .and_then(|pair| pair.split_once('='))
            .map(|(_, value)| value)
            .expect("cookie value");

        let restarted = AuthState::with_store(mode, None, store);
        let headers = {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::COOKIE,
                format!("{SESSION_COOKIE}={raw_cookie}")
                    .parse()
                    .expect("cookie header"),
            );
            headers
        };
        let subject = restarted
            .resolve(&headers)
            .await
            .expect("durable session loads")
            .expect("authenticated subject");
        assert_eq!(subject.as_str(), "subject-a");
        let fact = restarted
            .store
            .subject(&subject)
            .await
            .expect("durable subject loads")
            .expect("subject fact");
        assert_eq!(fact.profile.display_name.as_deref(), Some("Subject A"));
        assert!(fact
            .authorization
            .allows(layer2_core::Capability::CreateSession));
        let tokens = fact.tokens.expect("tokens persist as part of the fact");
        assert_eq!(tokens.access_token.unwrap().expose(), "access-a");
        assert_eq!(
            tokens.refresh_token.as_ref().map(|value| value.expose()),
            Some("refresh-a")
        );
        server.abort();
    }

    #[test]
    fn urlencoding_escapes_what_a_redirect_uri_needs() {
        assert_eq!(urlencode("http://a/b?c=d"), "http%3A%2F%2Fa%2Fb%3Fc%3Dd");
        assert_eq!(urlencode("plain-Value_1.0~"), "plain-Value_1.0~");
    }

    #[test]
    fn login_return_targets_are_only_canonical_workflow_paths() {
        let workflow_id = "123e4567-e89b-12d3-a456-426614174000";
        for valid in [
            format!("/triggers/{workflow_id}"),
            format!("/launches/{workflow_id}"),
            format!("/launches/{workflow_id}/recover"),
            format!("/launches/{workflow_id}/contribute"),
            "/sessions/sesn_trg_123e4567e89b12d3a456426614174000".to_string(),
            "/sessions/legacy-session_1.0~".to_string(),
        ] {
            assert_eq!(app_return_target(&valid).as_deref(), Ok(valid.as_str()));
        }

        for invalid in [
            "/",
            "/triggers/not-a-uuid",
            "/triggers/123e4567-e89b-12d3-a456-426614174000/extra",
            "/launches/not-a-uuid",
            "/launches/123e4567-e89b-12d3-a456-426614174000/delete",
            "/launches/123e4567-e89b-12d3-a456-426614174000/recover/extra",
            "/sessions/",
            "/sessions/..",
            "/sessions/session/extra",
            "/sessions/session%2Fextra",
            "/sessions/session one",
            "//example.com/triggers/123e4567-e89b-12d3-a456-426614174000",
            "https://example.com/triggers/123e4567-e89b-12d3-a456-426614174000",
            "/triggers/123e4567-e89b-12d3-a456-426614174000?next=https://example.com",
            "/launches/123e4567-e89b-12d3-a456-426614174000#fragment",
            "/sessions/session\\extra",
        ] {
            assert!(app_return_target(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[tokio::test]
    async fn login_endpoint_restores_each_owner_workflow_route() {
        let auth = AuthState::new(
            AuthMode::Trusted {
                subject: "local-developer".into(),
            },
            None,
        );
        let workflow_id = "123e4567-e89b-12d3-a456-426614174000";
        for return_to in [
            format!("/launches/{workflow_id}"),
            format!("/launches/{workflow_id}/recover"),
            format!("/launches/{workflow_id}/contribute"),
            "/sessions/sesn_trg_123e4567e89b12d3a456426614174000".to_string(),
        ] {
            let response = login_route(
                State(Arc::clone(&auth)),
                Query(LoginParams {
                    return_to: Some(return_to.clone()),
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(response.headers()[header::LOCATION], return_to);
        }
    }

    #[tokio::test]
    async fn login_endpoint_rejects_a_non_workflow_return_target() {
        let response = login_route(
            State(AuthState::new(
                AuthMode::Trusted {
                    subject: "local-developer".into(),
                },
                None,
            )),
            Query(LoginParams {
                return_to: Some("/developer".into()),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let problem = body_of(response).await;
        assert_eq!(problem["title"], "Invalid sign-in return target");
        assert_eq!(
            problem["detail"],
            "Only a canonical CMA workflow path may be restored."
        );
    }

    #[test]
    fn permissions_are_read_from_whichever_document_carries_them() {
        // The same reader serves userinfo and introspection, because the claim
        // is the claim wherever it appears.
        let body = serde_json::json!({ "permissions": ["cma:session:use", "*"] });
        assert_eq!(
            permissions_claim(&body),
            vec!["cma:session:use".to_string(), "*".to_string()]
        );
    }

    #[test]
    fn a_document_without_permissions_yields_none_rather_than_failing() {
        // Exactly the shape this provider's userinfo returns for a token that
        // was never granted a scope asking for roles. Reading it as empty is
        // what made every such caller unauthorized.
        let body = serde_json::json!({ "sub": "s", "email": "a@b.c", "name": "A" });
        assert!(permissions_claim(&body).is_empty());
        // A non-array, and entries that are not strings, are equally not
        // permissions and must not panic or half-read.
        assert!(permissions_claim(&serde_json::json!({ "permissions": "all" })).is_empty());
        assert_eq!(
            permissions_claim(&serde_json::json!({ "permissions": [1, "cma:session:use"] })),
            vec!["cma:session:use".to_string()]
        );
    }

    #[tokio::test]
    async fn a_provider_offering_no_introspection_falls_back_rather_than_denying() {
        // None is the signal to read userinfo instead. Returning an empty list
        // here would deny every caller against a provider that simply does not
        // offer the endpoint.
        let meta = ProviderMetadata {
            authorization_endpoint: "http://provider.invalid/authorize".into(),
            token_endpoint: "http://provider.invalid/token".into(),
            userinfo_endpoint: Some("http://provider.invalid/userinfo".into()),
            introspection_endpoint: None,
        };
        let found = introspect_permissions(&provider_auth(), &meta, "t")
            .await
            .expect("no endpoint is not an error");
        assert!(
            found.is_none(),
            "absent endpoint must not read as no grants"
        );
    }
}
