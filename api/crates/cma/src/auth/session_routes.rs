//! What a signed-in browser can ask about itself, and how it signs out.
//!
//! Split from the flow so that file is about getting somebody signed in and
//! this one about the two things they do afterwards. Both are short, and both
//! are read by whoever is writing a frontend rather than by whoever is
//! debugging the dance.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

use super::{session_cookie, AuthState, SESSION_COOKIE};

pub(super) async fn session(State(auth): State<Arc<AuthState>>, headers: HeaderMap) -> Response {
    match auth.resolve(&headers).await {
        Some(subject) => {
            // The identifier stays, because clients key on it; the display
            // details ride alongside so nothing has to render a UUID at a
            // person.
            let profile = auth.profiles.read().await.get(&subject).cloned();
            // What this caller may do, so a client can offer only what will be
            // permitted rather than guessing from a role name it has no
            // business knowing. Advisory: every one of these is checked again
            // by the core on the request that uses it.
            let entitled = auth.effective_authorization(&subject).await;
            let mut body = serde_json::json!({
                "authenticated": true,
                "subject": subject.as_str(),
                "capabilities": super::entitlements::reported(&entitled),
            });
            if let (Some(profile), Some(map)) = (profile, body.as_object_mut()) {
                if let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(&profile) {
                    map.extend(fields);
                }
            }
            Json(body).into_response()
        }
        None => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "authenticated": false })),
        )
            .into_response(),
    }
}

pub(super) async fn logout(State(auth): State<Arc<AuthState>>, headers: HeaderMap) -> Response {
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

pub(super) fn urlencode(value: &str) -> String {
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
