//! Browser handoff; the proof is displayed only in the authenticated browser.
use super::*;
use crate::auth::{session_routes::request_cookie, AUTH_COOKIE, AUTH_COOKIE_DOMAIN};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};

pub(crate) fn routes() -> Router<Arc<AuthState>> {
    Router::new()
        .route("/auth/bot-father/link/{link_id}", get(begin))
        .route("/auth/bot-father/link/{link_id}/complete", get(complete))
        .route("/auth/bot-father/link/{link_id}/revoke", post(revoke))
}
async fn begin(State(auth): State<Arc<AuthState>>, Path(link_id): Path<String>) -> Response {
    match auth.link_record(&link_id).await {
        Ok(row)
            if !row.active && !row.revoked && row.subject.is_none() && row.expires_at > now() =>
        {
            crate::auth::flow::actor_login(auth, &link_id).await
        }
        _ => StatusCode::GONE.into_response(),
    }
}

async fn revoke(
    State(auth): State<Arc<AuthState>>,
    Path(link_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        if headers.contains_key(header::AUTHORIZATION) {
            return Err(BotError::Forbidden);
        }
        let AuthMode::Provider { public_url, .. } = &auth.mode else {
            return Err(BotError::Forbidden);
        };
        let expected = reqwest::Url::parse(public_url)
            .map_err(|_| BotError::Forbidden)?
            .origin()
            .ascii_serialization();
        if headers
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            != Some(expected.as_str())
        {
            return Err(BotError::Forbidden);
        }
        let request = auth
            .resolve_request(&headers)
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        let row = auth.link_record(&link_id).await?;
        if row.subject.as_ref() != Some(&request.subject) || request.delegated_credential.is_none()
        {
            return Err(BotError::Forbidden);
        }
        auth.revoke_actor_link(&row.actor, &link_id).await
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::FORBIDDEN.into_response(),
    }
}
async fn complete(
    State(auth): State<Arc<AuthState>>,
    Path(link_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match complete_candidate(&auth, &link_id, &headers).await {
        Ok((code, profile)) => {
            let name = escape(
                profile
                    .display_name
                    .as_deref()
                    .unwrap_or("your NyxID account"),
            );
            let action = auth.bot_father_username.as_ref().map(|name| format!("<p><a href=\"https://t.me/{name}?start=link_{code}\">Return to Telegram and confirm</a></p>")).unwrap_or_default();
            let html = format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Connect CMA Bot Father</title><body><main><h1>Confirm your CMA Bot Father connection</h1><p>Signed in as {name}. Return to the Telegram conversation that started this sign-in to authorize Bot Father to create and manage bots for this account.</p>{action}<p>Alternatively, send this code in that conversation:</p><pre>link_{code}</pre><p>No bot or sandbox has been created. Only confirm if you started this request.</p></main></body></html>");
            let mut response = Html(html).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
            response
                .headers_mut()
                .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
            response.headers_mut().insert(
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; base-uri 'none'; frame-ancestors 'none'"
                    .parse()
                    .unwrap(),
            );
            response
        }
        Err(_) => (
            StatusCode::FORBIDDEN,
            "This connection expired or was already used. Start again from CMA Bot Father.",
        )
            .into_response(),
    }
}
async fn complete_candidate(
    auth: &AuthState,
    link_id: &str,
    headers: &HeaderMap,
) -> Result<(String, Profile), BotError> {
    if headers.contains_key(header::AUTHORIZATION) {
        return Err(BotError::Forbidden);
    }
    let cookie = request_cookie(headers, AUTH_COOKIE).ok_or(BotError::Forbidden)?;
    let login = auth
        .store
        .browser_login(&SecretDigest::identify(AUTH_COOKIE_DOMAIN, cookie))
        .await
        .map_err(|_| BotError::Unavailable)?
        .filter(|login| login.expires_at > SystemTime::now())
        .ok_or(BotError::Forbidden)?;
    let state = auth
        .store
        .subject(&login.subject)
        .await
        .map_err(|_| BotError::Unavailable)?
        .ok_or(BotError::Forbidden)?;
    state.validate().map_err(|_| BotError::Unavailable)?;
    auth.bot_owner_token_fenced(&login.subject, Some(&state.incarnation))
        .await?;
    let mut row = auth.link_record(link_id).await?;
    if row.subject.is_some() || row.active || row.revoked || row.expires_at <= now() {
        return Err(BotError::Forbidden);
    }
    let code = uuid::Uuid::new_v4().simple().to_string();
    let revision = row.revision;
    row.subject = Some(login.subject);
    row.incarnation = Some(state.incarnation);
    row.profile = state.profile.clone();
    row.confirmation_digest = Some(digest("actor-link-confirmation", &code));
    row.revision += 1;
    if !auth
        .store
        .replace_actor_link(Some(revision), row)
        .await
        .map_err(|_| BotError::Unavailable)?
    {
        return Err(BotError::Conflict);
    }
    Ok((code, state.profile))
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
