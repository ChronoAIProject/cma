//! The authorization-code flow: redirect out, exchange on the way back.
//!
//! Split from the module that owns auth state so each file stays about one
//! thing: this one is the browser-facing dance, `super` is what the rest of
//! the service asks "who is calling".

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use layer2_core::SubjectId;
use serde::Deserialize;

use super::permissions::{introspect_permissions, permissions_claim};
use super::persistence::{DurableSubjectTokens, LoginAttempt, SecretDigest, SecretValue};
use super::session_routes::{logout, session, urlencode};
use super::{code_challenge, random_token, AuthMode, AuthState, ProviderMetadata, SESSION_COOKIE};

const LOGIN_ATTEMPT_DOMAIN: &str = "login-attempt";
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

/// Routes for the login flow.
pub fn router(auth: Arc<AuthState>) -> Router {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/session", get(session))
        .route("/auth/logout", get(logout))
        .with_state(auth)
}

async fn login(State(auth): State<Arc<AuthState>>) -> Response {
    match &auth.mode {
        AuthMode::Trusted { .. } => Redirect::to("/").into_response(),
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
                let challenge = code_challenge(&verifier);
                if let Err(error) = auth
                    .store
                    .put_login_attempt(LoginAttempt {
                        id: SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, &state),
                        verifier: SecretValue::new(verifier),
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
                Redirect::to(&url).into_response()
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

    // A callback whose state we did not issue is not ours to complete.
    let verifier = match params.state.as_ref() {
        Some(state) => {
            let id = SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, state);
            match auth.store.take_login_attempt(&id).await {
                Ok(Some(attempt)) if attempt.expires_at > SystemTime::now() => attempt.verifier,
                Ok(Some(_)) | Ok(None) => {
                    tracing::warn!("callback carried an unknown state");
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "title": "Unrecognised login attempt" })),
                    )
                        .into_response();
                }
                Err(error) => {
                    tracing::error!(error = %error, "could not consume a login attempt");
                    return layer1_public::auth::StoreUnavailable.into_response();
                }
            }
        }
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "title": "Missing state parameter" })),
            )
                .into_response()
        }
    };

    // Every callback attempt consumes its state, including a provider refusal
    // or malformed response. Otherwise the same authorization attempt remains
    // replayable until TTL cleanup merely because this branch needed no code.
    if let Some(err) = params.error {
        tracing::warn!(error = %err, "provider returned an error on callback");
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "title": "Login failed", "detail": err })),
        )
            .into_response();
    }
    let Some(code) = params.code else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "title": "Missing authorization code" })),
        )
            .into_response();
    };

    let meta = match auth.metadata().await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "title": "Discovery failed", "detail": e })),
            )
                .into_response()
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
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "title": "Token exchange failed" })),
            )
                .into_response();
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "token endpoint rejected the exchange");
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "title": "Token exchange rejected" })),
        )
            .into_response();
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
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "title": "Token response not usable" })),
            )
                .into_response();
        }
    };

    let (subject, permissions, profile) =
        match resolve_subject(&auth, &meta, &token.access_token).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "cannot resolve subject");
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "title": "Cannot resolve subject", "detail": e })),
                )
                    .into_response();
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
                access_token: SecretValue::new(token.access_token),
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
        return layer1_public::auth::StoreUnavailable.into_response();
    }
    if !has_refresh_token {
        // Without one, a long session ends when the access token does. Say so
        // once, at the point the decision is visible.
        tracing::warn!(
            %subject,
            "provider issued no refresh token; delegated access will expire and cannot be renewed"
        );
    }

    // Capabilities are resolved once, at sign-in, from the permissions the
    // provider reported. A caller with none authenticates successfully and is
    // then refused every operation, which is the correct outcome for someone
    // who has a platform account but no access here.
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
            return layer1_public::auth::StoreUnavailable.into_response();
        }
    };
    // Host-only, HttpOnly, SameSite=Lax: the cookie is never readable from
    // script, and never sent on a cross-site request that could act as the user.
    // Production HTTPS also marks it Secure; explicit HTTP is retained for
    // local development where a Secure cookie would never be returned.
    let cookie = issued_session_cookie(&session, auth.secure_cookies());
    (
        StatusCode::SEE_OTHER,
        [(header::SET_COOKIE, cookie), (header::LOCATION, "/".into())],
    )
        .into_response()
}

fn issued_session_cookie(session: &str, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={session}; Path=/; HttpOnly{secure}; SameSite=Lax; Max-Age=86400")
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
    // Captured at sign-in because this is the only moment the provider's own
    // account details are in hand; nothing later has a token to ask with.
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

    #[tokio::test]
    async fn the_identity_answer_says_what_this_caller_may_do() {
        // Without this a client has to guess, and the only things it could
        // guess from are a role name it should never see or a refusal it would
        // have to provoke.
        let auth = provider_auth();
        let subject = SubjectId("subject-a".into());
        auth.store_authorization(
            &subject,
            layer2_core::Authorization::from_capabilities([
                layer2_core::Capability::ReadAllSessions,
                layer2_core::Capability::ControlAnySession,
            ]),
        )
        .await
        .expect("authorization persists");
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
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body_of(response).await["code"], "store_failed");
    }

    #[tokio::test]
    async fn expired_callback_state_is_consumed_but_never_accepted() {
        let store = Arc::new(MemoryAuthStore::default());
        let state = "opaque-state";
        store
            .put_login_attempt(LoginAttempt {
                id: SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, state),
                verifier: SecretValue::new("verifier".into()),
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
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(store
            .take_login_attempt(&SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, state))
            .await
            .expect("store works")
            .is_none());
    }

    #[tokio::test]
    async fn a_provider_error_callback_still_consumes_its_state() {
        let store = Arc::new(MemoryAuthStore::default());
        let state = "refused-state";
        let id = SecretDigest::identify(LOGIN_ATTEMPT_DOMAIN, state);
        store
            .put_login_attempt(LoginAttempt {
                id: id.clone(),
                verifier: SecretValue::new("verifier".into()),
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
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(store
            .take_login_attempt(&id)
            .await
            .expect("store works")
            .is_none());
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

        let started = login(State(first)).await;
        assert_eq!(started.status(), StatusCode::SEE_OTHER);
        let location = started
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("authorization redirect");
        let redirect = reqwest::Url::parse(location).expect("redirect is a URL");
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
        )
        .await;
        assert_eq!(completed.status(), StatusCode::SEE_OTHER);
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
        assert_eq!(tokens.access_token.expose(), "access-a");
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
