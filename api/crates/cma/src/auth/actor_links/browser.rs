//! Authenticated explicit delegation, with legacy chat-proof compatibility.
use super::*;
use crate::auth::{session_routes::request_cookie, AUTH_COOKIE, AUTH_COOKIE_DOMAIN};
use axum::{
    extract::{Form, OriginalUri, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};

pub(crate) fn routes() -> Router<Arc<AuthState>> {
    Router::new()
        .route("/auth/bot-father/link/{link_id}", get(begin))
        .route("/auth/bot-father/link/{link_id}/complete", get(complete))
        .route("/auth/bot-father/link/{link_id}/consent", post(consent))
        .route("/auth/bot-father/link/{link_id}/revoke", post(revoke))
}
async fn begin(
    State(auth): State<Arc<AuthState>>,
    Path(link_id): Path<String>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    if let Some(redirect) = auth.canonical_login_redirect(&headers, &uri) {
        return redirect;
    }
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
        if !auth.permits_browser_origin(&headers) {
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
    match auth.link_record(&link_id).await {
        Ok(row) if row.activation == ActorActivation::BrowserConsent => {
            return match auth.actor_browser_review(&link_id, &headers).await {
                Ok(row) => browser_card(&auth, &row),
                Err(_) => rejected(),
            };
        }
        Ok(_) => {}
        Err(_) => return rejected(),
    }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsentForm {
    csrf_token: String,
    subject: String,
    review_revision: u64,
}
async fn consent(
    State(auth): State<Arc<AuthState>>,
    Path(link_id): Path<String>,
    headers: HeaderMap,
    Form(form): Form<ConsentForm>,
) -> Response {
    match auth
        .approve_actor_browser_consent(
            &link_id,
            &headers,
            &form.csrf_token,
            &form.subject,
            form.review_revision,
        )
        .await
    {
        Ok(_) => match auth.actor_browser_review(&link_id, &headers).await {
            Ok(row) => browser_card(&auth, &row),
            Err(_) => rejected(),
        },
        Err(_) => rejected(),
    }
}

fn browser_card(auth: &AuthState, row: &ActorLinkRecord) -> Response {
    let Some(subject) = row.subject.as_ref() else {
        return rejected();
    };
    let Some(evidence) = row.browser_consent.as_ref() else {
        return rejected();
    };
    let name = escape(
        row.profile
            .display_name
            .as_deref()
            .unwrap_or("NyxID account"),
    );
    let subject = escape(subject.as_str());
    let actor = escape(&row.actor.actor_id);
    let conversation = escape(&row.actor.conversation_id);
    let platform = escape(&row.actor.platform);
    let channel = escape(&row.actor.channel_id);
    let content = if row.active {
        let return_link = auth
            .bot_father_username
            .as_ref()
            .map(|username| {
                format!("<p><a href=\"https://t.me/{username}\">Return to Telegram</a></p>")
            })
            .unwrap_or_default();
        format!("<h1>Account connected</h1><p>{name} ({subject}) authorized the {platform} account {actor} in conversation {conversation}.</p><p>Your assistant will continue in the original conversation. No code or extra chat message is needed.</p>{return_link}")
    } else {
        format!("<h1>Authorize your CMA assistant</h1><p>Signed in as <strong>{name}</strong>.</p><dl><dt>NyxID account</dt><dd>{subject}</dd><dt>Messaging platform</dt><dd>{platform}</dd><dt>Messaging account ID</dt><dd>{actor}</dd><dt>Conversation ID</dt><dd>{conversation}</dd><dt>Channel ID</dt><dd>{channel}</dd></dl><p>Allow the messaging account above to use your CMA account through this assistant: manage profiles, sandboxes, workspaces, agent sessions, and bots, within your current personal and organization permissions.</p><p>This authorizes the displayed messaging account. Only continue if you trust it. Signing in alone does not grant this access or create resources.</p><form method=\"post\" action=\"/auth/bot-father/link/{}/consent\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><input type=\"hidden\" name=\"subject\" value=\"{subject}\"><input type=\"hidden\" name=\"review_revision\" value=\"{}\"><button type=\"submit\">Authorize and continue in Telegram</button></form>", row.link_id, evidence.csrf_token, evidence.review_revision)
    };
    let mut response = Html(format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Connect your CMA account</title><body><main>{content}</main></body></html>")).into_response();
    for (name, value) in [
        (header::CACHE_CONTROL, "no-store"),
        // A no-referrer policy makes navigation POSTs carry Origin: null,
        // which our consent endpoint correctly rejects. Preserve the origin
        // for this same-origin form while suppressing cross-origin referrers.
        (header::REFERRER_POLICY, "same-origin"),
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
    ] {
        response.headers_mut().insert(name, value.parse().unwrap());
    }
    response
}

fn rejected() -> Response {
    (StatusCode::FORBIDDEN, "This connection is expired, revoked, or belongs to another sign-in. Start again from your CMA assistant.").into_response()
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
    if row.activation != ActorActivation::LegacyChatProof
        || row.subject.is_some()
        || row.active
        || row.revoked
        || row.expires_at <= now()
    {
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
