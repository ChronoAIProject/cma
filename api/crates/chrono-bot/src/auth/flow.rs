//! The authorization-code flow: redirect out, exchange on the way back.
//!
//! Split from the module that owns auth state so each file stays about one
//! thing: this one is the browser-facing dance, `super` is what the rest of
//! the service asks "who is calling".

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use layer2_core::SubjectId;
use serde::Deserialize;

use super::{
    code_challenge, session_cookie, uuid_v4, AuthMode, AuthState, ProviderMetadata, SESSION_COOKIE,
};

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
                let verifier = uuid_v4();
                let state = uuid_v4();
                auth.pending
                    .write()
                    .await
                    .insert(state.clone(), verifier.clone());
                let url = format!(
                    "{}?response_type=code&client_id={}&redirect_uri={}&scope=openid%20profile\
&state={}&code_challenge={}&code_challenge_method=S256",
                    meta.authorization_endpoint,
                    urlencode(client_id),
                    urlencode(&redirect_uri),
                    urlencode(&state),
                    urlencode(&code_challenge(&verifier)),
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
        Some(state) => match auth.pending.write().await.remove(state) {
            Some(v) => v,
            None => {
                tracing::warn!("callback carried an unknown state");
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "title": "Unrecognised login attempt" })),
                )
                    .into_response();
            }
        },
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "title": "Missing state parameter" })),
            )
                .into_response()
        }
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
        ("code_verifier", verifier.as_str()),
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

    let subject = match resolve_subject(&auth, &meta, &token.access_token).await {
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

    let session = auth.issue_session(subject).await;
    // Host-only, HttpOnly, SameSite=Lax: the cookie is never readable from
    // script, and never sent on a cross-site request that could act as the user.
    let cookie =
        format!("{SESSION_COOKIE}={session}; Path=/; HttpOnly; SameSite=Lax; Max-Age=86400");
    (
        StatusCode::SEE_OTHER,
        [(header::SET_COOKIE, cookie), (header::LOCATION, "/".into())],
    )
        .into_response()
}

async fn resolve_subject(
    auth: &AuthState,
    meta: &ProviderMetadata,
    access_token: &str,
) -> Result<SubjectId, String> {
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
    body.get("sub")
        .and_then(|v| v.as_str())
        .map(|s| SubjectId(s.to_string()))
        .ok_or_else(|| "userinfo carried no subject claim".into())
}

async fn session(State(auth): State<Arc<AuthState>>, headers: HeaderMap) -> Response {
    match auth.resolve(&headers).await {
        Some(subject) => Json(serde_json::json!({
            "authenticated": true,
            "subject": subject.as_str(),
        }))
        .into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "authenticated": false })),
        )
            .into_response(),
    }
}

async fn logout(State(auth): State<Arc<AuthState>>, headers: HeaderMap) -> Response {
    if let Some(token) = session_cookie(&headers) {
        auth.sessions.write().await.remove(&token);
    }
    (
        StatusCode::SEE_OTHER,
        [
            (
                header::SET_COOKIE,
                format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
            ),
            (header::LOCATION, "/".into()),
        ],
    )
        .into_response()
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencoding_escapes_what_a_redirect_uri_needs() {
        assert_eq!(urlencode("http://a/b?c=d"), "http%3A%2F%2Fa%2Fb%3Fc%3Dd");
        assert_eq!(urlencode("plain-Value_1.0~"), "plain-Value_1.0~");
    }
}
