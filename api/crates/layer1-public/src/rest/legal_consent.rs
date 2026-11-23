//! Owner acceptance of the deployment's published terms and privacy notice.
use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::{
    account_deletion::OwnerSessionAuthority,
    legal_consent::{LegalConsentError, LegalConsentService, LegalConsentStatus},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::idempotency::RequestBody;
use crate::{AuthMethod, AuthenticatedCaller};

const PATH: &str = "/api/v1/account/legal-consent";

pub fn legal_consent_router(service: Arc<LegalConsentService>) -> Router {
    Router::new()
        .route(PATH, get(status).put(accept))
        .with_state(service)
        .layer(axum::middleware::map_response(private_no_store))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct LegalConsentAcceptance {
    /// The exact published version the owner reviewed.
    version: String,
}

/// The published documents and the owner's acceptance of this version.
#[derive(Serialize, ToSchema)]
struct LegalConsent {
    version: String,
    terms_url: String,
    privacy_url: String,
    /// When this owner accepted this version; null until accepted.
    #[schema(required = true)]
    accepted_at: Option<String>,
}

impl From<LegalConsentStatus> for LegalConsent {
    fn from(value: LegalConsentStatus) -> Self {
        Self {
            version: value.terms.version().to_owned(),
            terms_url: value.terms.terms_url().to_owned(),
            privacy_url: value.terms.privacy_url().to_owned(),
            accepted_at: value.accepted_at,
        }
    }
}

/// Read the current terms and whether the verified owner has accepted them.
#[utoipa::path(get, path = "/api/v1/account/legal-consent", tag = "legal consent",
    responses((status = 200, body = LegalConsent), (status = 401), (status = 403),
        (status = 404), (status = 503)))]
async fn status(
    State(service): State<Arc<LegalConsentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    method: Option<Extension<AuthMethod>>,
    authority: Option<Extension<OwnerSessionAuthority>>,
) -> Response {
    if method.is_some() {
        return legal_consent_problem(LegalConsentError::RequiresOwnerSession);
    }
    match service
        .status(&caller, authority.as_ref().map(|Extension(value)| value))
        .await
    {
        Ok(status) => Json(LegalConsent::from(status)).into_response(),
        Err(error) => legal_consent_problem(error),
    }
}

/// Record acceptance of the current version; repeating it keeps the original time.
#[utoipa::path(put, path = "/api/v1/account/legal-consent", tag = "legal consent",
    request_body = LegalConsentAcceptance,
    responses((status = 200, body = LegalConsent), (status = 400), (status = 401),
        (status = 403), (status = 404), (status = 409), (status = 415), (status = 422),
        (status = 503)))]
async fn accept(
    State(service): State<Arc<LegalConsentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    method: Option<Extension<AuthMethod>>,
    authority: Option<Extension<OwnerSessionAuthority>>,
    body: RequestBody<LegalConsentAcceptance>,
) -> Response {
    if method.is_some() {
        return legal_consent_problem(LegalConsentError::RequiresOwnerSession);
    }
    let body = match body.into_parts().0 {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    match service
        .accept(
            &caller,
            authority.as_ref().map(|Extension(value)| value),
            &body.version,
        )
        .await
    {
        Ok(status) => Json(LegalConsent::from(status)).into_response(),
        Err(error) => legal_consent_problem(error),
    }
}

fn legal_consent_problem(error: LegalConsentError) -> Response {
    use LegalConsentError::*;
    let (status, slug, code, detail) = match error {
        NotConfigured => (
            StatusCode::NOT_FOUND,
            "legal-terms-not-configured",
            "legal_terms_not_configured",
            "This deployment publishes no terms that require acceptance.",
        ),
        RequiresOwnerSession => (
            StatusCode::FORBIDDEN,
            "legal-consent-requires-owner-session",
            "legal_consent_requires_owner_session",
            "Legal consent requires a verified human owner session.",
        ),
        VersionMismatch => (
            StatusCode::CONFLICT,
            "legal-terms-version-mismatch",
            "legal_terms_version_mismatch",
            "Review the current terms version before accepting it.",
        ),
        SignInRequired => (
            StatusCode::CONFLICT,
            "legal-consent-sign-in-required",
            "legal_consent_sign_in_required",
            "Sign out and sign in again before accepting the terms.",
        ),
        Store(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "store-failed",
            "store_failed",
            "Legal consent is temporarily unavailable.",
        ),
    };
    super::problem_response(status, super::problem_document(status, slug, detail, code))
}

// Include extractor refusals in the privacy policy.
async fn private_no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(status, accept),
    components(schemas(LegalConsentAcceptance, LegalConsent))
)]
struct LegalConsentApi;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    LegalConsentApi::openapi()
}

#[cfg(test)]
#[path = "legal_consent_tests.rs"]
mod tests;
