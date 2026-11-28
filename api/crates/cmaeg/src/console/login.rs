//! NyxID sign-in: the authorization code flow with PKCE S256 as a
//! confidential client. It starts only from the explicit sign-in link, its
//! state travels in a sealed cookie so either pod can finish it, and the
//! callback checks that cookie before any NyxID call.
use std::time::Duration;

use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use cmaeg_core::seal::{open_console_login, seal_console_login, ConsoleLogin, CONSOLE_LOGIN_TTL};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::{
    cookies,
    pages::{self, signin},
    redirect::{self, HOME},
    subject_hash_prefix,
    token::TokenFailure,
    Console,
};
use crate::{auth::CreatorAuthError, telemetry::names};

const VERIFIER_BYTES: usize = 64;
const STATE_BYTES: usize = 32;
/// How long a callback waits for one of the four exchange permits.
const EXCHANGE_WAIT: Duration = Duration::from_secs(5);
const MAX_CODE_BYTES: usize = 2_048;

pub(crate) const EXPIRED: &str = "Sign-in expired; please try again.";
const CANCELLED: &str = "Sign-in was cancelled or refused; please try again.";
const FAILED: &str = "Sign-in failed; please try again.";

#[derive(Deserialize)]
pub(crate) struct StartQuery {
    return_to: Option<String>,
}

/// `GET /console/login`: reached only from the sign-in link, never a form.
pub(crate) async fn start(
    State(console): State<Console>,
    query: Result<Query<StartQuery>, QueryRejection>,
) -> Response {
    let return_to = redirect::return_to(
        query
            .as_ref()
            .ok()
            .and_then(|Query(query)| query.return_to.as_deref()),
    );
    if let Some(refused) = console.limiter().refusal() {
        return refused;
    }
    let Some(endpoints) = console.endpoints() else {
        return pages::unavailable();
    };
    let (Some(verifier), Some(state)) = (random(VERIFIER_BYTES), random(STATE_BYTES)) else {
        tracing::error!("secure randomness unavailable; console sign-in not started");
        return pages::unavailable();
    };
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let login = ConsoleLogin {
        state,
        verifier,
        return_to,
        exp: unix_now().saturating_add(CONSOLE_LOGIN_TTL),
    };
    let Some(cookie) = seal_console_login(console.keys(), &login)
        .ok()
        .and_then(|sealed| {
            cookies::set(
                cookies::LOGIN,
                &sealed,
                u64::try_from(CONSOLE_LOGIN_TTL).ok(),
            )
        })
    else {
        return pages::unavailable();
    };
    // No `resource` and no `requested_service_ids`: the token is for
    // identity only.
    let mut target = endpoints.authorization.clone();
    target
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", console.client_id())
        .append_pair("redirect_uri", &console.redirect_uri())
        .append_pair("scope", "openid profile")
        .append_pair("state", &login.state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    count("started");
    let mut response = StatusCode::FOUND.into_response();
    let headers = response.headers_mut();
    match HeaderValue::from_str(target.as_str()) {
        Ok(location) => headers.insert(header::LOCATION, location),
        Err(_) => return pages::unavailable(),
    };
    headers.insert(header::SET_COOKIE, cookie);
    response
}

#[derive(Deserialize)]
pub(crate) struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `GET /console/callback?code&state`.
pub(crate) async fn callback(
    State(console): State<Console>,
    headers: HeaderMap,
    query: Result<Query<CallbackQuery>, QueryRejection>,
) -> Response {
    let query = query.map_or(
        CallbackQuery {
            code: None,
            state: None,
            error: None,
        },
        |Query(query)| query,
    );
    if let Some(refused) = console.limiter().refusal() {
        return refused;
    }
    // Before any NyxID call: a request without a cookie minted by
    // /console/login, or with another state, costs no outbound call.
    let Some(login) = cookies::get(&headers, cookies::LOGIN)
        .and_then(|sealed| open_console_login(console.keys(), sealed, unix_now()).ok())
    else {
        return refuse("expired", HOME, EXPIRED);
    };
    let (Some(code), Some(state)) = (query.code, query.state) else {
        let notice = if query.error.is_some() {
            CANCELLED
        } else {
            EXPIRED
        };
        return refuse("denied", &login.return_to, notice);
    };
    if !bool::from(state.as_bytes().ct_eq(login.state.as_bytes()))
        || code.is_empty()
        || code.len() > MAX_CODE_BYTES
    {
        return refuse("expired", &login.return_to, EXPIRED);
    }
    let Some(endpoints) = console.endpoints() else {
        return clear_login(pages::unavailable());
    };
    let exchange_started = std::time::Instant::now();
    let tokens = {
        let Ok(Ok(_permit)) = tokio::time::timeout(EXCHANGE_WAIT, console.exchange_permit()).await
        else {
            count("unavailable");
            return clear_login(pages::unavailable());
        };
        console
            .tokens()
            .exchange(
                &endpoints.token,
                &code,
                &login.verifier,
                &console.redirect_uri(),
            )
            .await
    };
    let mut tokens = match tokens {
        Ok(tokens) => tokens,
        Err(TokenFailure::Definitive) => {
            tracing::warn!(
                stage = "token_exchange",
                reason = "grant_refused",
                "console sign-in rejected"
            );
            return refuse("failed", &login.return_to, FAILED);
        }
        Err(TokenFailure::Transient) => {
            count("unavailable");
            return clear_login(pages::unavailable());
        }
    };
    // The id_token is discarded unread: identity is NyxID's users/me, and
    // PKCE plus state already bind the code to this browser. REST's rule and
    // its 3 s timeout name the session subject, and fill REST's per-token
    // cache exactly as a REST call with this token would.
    let creator = match console
        .creator_auth()
        .authenticate_token(tokens.access.as_str(), None)
        .await
    {
        Ok(creator) => creator,
        Err(error) => {
            console.sessions().revoke_unused(tokens.refresh);
            return match error {
                CreatorAuthError::NotAuthenticated | CreatorAuthError::Forbidden => {
                    tracing::warn!(
                        stage = "identity",
                        reason = "not_authenticated",
                        "console sign-in rejected"
                    );
                    refuse("failed", &login.return_to, FAILED)
                }
                CreatorAuthError::ProviderUnavailable => {
                    count("unavailable");
                    clear_login(pages::unavailable())
                }
            };
        }
    };
    // Anchor the provider lifetime before exchange/identity I/O. Durable
    // creation additionally reserves its bounded database operation budget.
    if crate::request_audit::bind(
        &creator.subject,
        None,
        cmaeg_core::request_audit::Credential::WebSession,
    )
    .await
    .is_err()
    {
        console.sessions().revoke_unused(tokens.refresh);
        return clear_login(pages::unavailable());
    }
    tokens.expires_in = tokens.expires_in.saturating_sub(exchange_started.elapsed());
    let subject_hash = subject_hash_prefix(&creator.subject);
    let Ok((sid, _)) = console
        .sessions()
        .create_session(creator.subject, tokens)
        .await
    else {
        count("unavailable");
        return clear_login(pages::unavailable());
    };
    count("ok");
    tracing::info!(subject_hash_prefix = %subject_hash, outcome = "ok", "console login");
    let mut response = clear_login(redirect::see_other(&login.return_to));
    if let Some(cookie) = cookies::set(cookies::SESSION, sid.as_str(), None) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

/// The sign-in page with `notice`, the login cookie cleared.
fn refuse(outcome: &'static str, return_to: &str, notice: &str) -> Response {
    count(outcome);
    tracing::info!(outcome, "console login");
    clear_login(signin::sign_in(
        StatusCode::BAD_REQUEST,
        return_to,
        Some(notice),
    ))
}

fn clear_login(mut response: Response) -> Response {
    response
        .headers_mut()
        .append(header::SET_COOKIE, cookies::clear(cookies::LOGIN));
    response
}

fn count(outcome: &'static str) {
    metrics::counter!(names::CONSOLE_LOGINS, names::OUTCOME => outcome).increment(1);
}

fn random(bytes: usize) -> Option<String> {
    let mut value = vec![0_u8; bytes];
    SystemRandom::new().fill(&mut value).ok()?;
    Some(URL_SAFE_NO_PAD.encode(value))
}

fn unix_now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
