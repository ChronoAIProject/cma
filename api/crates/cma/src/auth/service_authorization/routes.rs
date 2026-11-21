use axum::{
    extract::{Query, State},
    response::Html,
    routing::get,
    Extension, Router,
};
use layer1_public::AuthenticatedResourceCaller;
use layer2_core::account_deletion::OwnerSessionAuthority;

use super::*;

pub(crate) fn router(auth: Arc<AuthState>) -> Router {
    Router::new()
        .route("/auth/service-authorization", get(account))
        .route(
            CALLBACK_PATH,
            get(callback).layer(axum::middleware::map_response(
                |mut response: Response| async move {
                    response
                        .headers_mut()
                        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
                    response
                },
            )),
        )
        .with_state(auth)
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                if !response.headers().contains_key(header::CACHE_CONTROL) {
                    response.headers_mut().insert(
                        header::CACHE_CONTROL,
                        header::HeaderValue::from_static("no-store"),
                    );
                }
                response
            },
        ))
}

async fn account(
    State(auth): State<Arc<AuthState>>,
    AuthenticatedResourceCaller { context, .. }: AuthenticatedResourceCaller,
    authority: Option<Extension<OwnerSessionAuthority>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RecoveryError> {
    let provider = auth
        .service_authorization
        .as_ref()
        .ok_or(RecoveryError::Disabled)?;
    if context.credential_source != CredentialSource::Retained
        || authority
            .is_none_or(|Extension(authority)| authority.subject() != &context.caller.subject)
    {
        return Err(RecoveryError::Forbidden);
    }
    let subject = &context.caller.subject;
    let key = browser_key(&headers)?;
    live_session(&auth, &key, subject).await?;
    let grant = provider_evidence("account_grant", provider.account(subject))
        .await
        .filter(|grant| &grant.subject == subject);
    live_session(&auth, &key, subject).await?;
    let mut body = serde_json::json!({
        "owner": subject.as_str(),
        "status": grant.as_ref().map_or("unknown", |grant| {
            if grant.authorized { "authorized" } else { "revoked" }
        }),
    });
    if let Some(url) = grant
        .and_then(|grant| grant.management_url)
        .filter(|url| provider_url(&auth, url))
    {
        body["management_url"] = url.as_str().into();
    }
    Ok(Json(body))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Callback {
    state: String,
    code: Option<String>,
    error: Option<String>,
    iss: Option<String>,
    #[serde(rename = "error_description")]
    _description: Option<String>,
    #[serde(rename = "error_uri")]
    _error_uri: Option<String>,
}

async fn callback(
    State(auth): State<Arc<AuthState>>,
    headers: HeaderMap,
    Query(callback): Query<Callback>,
) -> Result<Response, RecoveryError> {
    let provider = auth
        .service_authorization
        .as_ref()
        .ok_or(RecoveryError::Disabled)?;
    let issuer = provider.issuer().ok_or(RecoveryError::Unknown)?;
    if callback.state.len() != 64
        || !callback.state.bytes().all(|byte| byte.is_ascii_hexdigit())
        || callback.code.is_some() == callback.error.is_some()
        || callback
            .code
            .as_ref()
            .is_some_and(|code| code.is_empty() || code.len() > 2048)
        || callback
            .iss
            .as_deref()
            .is_some_and(|iss| iss != issuer.as_str())
    {
        return Err(RecoveryError::Invalid);
    }
    let key = browser_key(&headers)?;
    let id = SecretDigest::identify(ATTEMPT_DOMAIN, &callback.state);
    let attempt = auth
        .store
        .take_login_attempt(&id, &key)
        .await
        .map_err(|_| RecoveryError::Unknown)?
        .ok_or(RecoveryError::Changed)?;
    let intent = attempt
        .service_authorization
        .ok_or(RecoveryError::Changed)?;
    let return_to = return_target(&intent.return_to)?;
    if attempt.expires_at <= SystemTime::now()
        || intent.issuer != issuer.as_str()
        || intent.requested.is_empty()
        || !intent.requested.is_subset(&intent.required)
    {
        return Err(RecoveryError::Changed);
    }
    live_session(&auth, &key, &intent.subject).await?;
    let outcome = match callback.code {
        Some(code) => complete(provider.as_ref(), &attempt.verifier, &intent, &code).await?,
        None if callback.error.as_deref() == Some("access_denied") => Outcome::Cancelled,
        None => Outcome::Unknown,
    };
    live_session(&auth, &key, &intent.subject).await?;
    Ok(outcome.page(&return_to))
}

async fn complete(
    provider: &dyn GrantAuthority,
    verifier: &SecretValue,
    intent: &RecoveryIntent,
    code: &str,
) -> Result<Outcome, RecoveryError> {
    let Some(identity) =
        provider_evidence("redeem_consent", provider.redeem(intent, code, verifier)).await
    else {
        return Ok(Outcome::Unknown);
    };
    if identity.subject != intent.subject || identity.issuer != intent.issuer {
        return Err(RecoveryError::Forbidden);
    }
    match provider_evidence("required_grant_readback", provider.missing(intent)).await {
        Some(missing) if missing.is_empty() => Ok(Outcome::Verified),
        Some(missing) if missing.is_subset(&intent.required) => Ok(Outcome::Incomplete),
        _ => Ok(Outcome::Unknown),
    }
}

enum Outcome {
    Verified,
    Incomplete,
    Cancelled,
    Unknown,
}

impl Outcome {
    fn page(self, return_to: &str) -> Response {
        let (code, message) = match self {
            Self::Verified => ("verified", "Service authorization verified"),
            Self::Incomplete => ("incomplete", "Required services still need authorization"),
            Self::Cancelled => ("cancelled", "Service authorization cancelled"),
            Self::Unknown => ("unknown", "Service authorization could not be verified"),
        };
        tracing::debug!(code, "Service authorization callback completed");
        let target = return_to
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        (
            [
                (header::CACHE_CONTROL, "private, no-store"),
                (header::CONTENT_SECURITY_POLICY, "default-src 'none'; base-uri 'none'; frame-ancestors 'none'"),
            ],
            Html(format!(
                "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>{message}</title><main><h1>{message}</h1><a href=\"{target}\">Return to draft</a></main></html>"
            )),
        ).into_response()
    }
}
