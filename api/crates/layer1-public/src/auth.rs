//! Extracting the authenticated principal from a request.
//!
//! Identity is asserted by the identity provider, never by this service. The
//! process that owns outbound network access verifies the credential and places
//! the resulting subject in request extensions; this module only reads it back.
//! Keeping verification out of the interface layer is what stops an interface
//! from quietly acquiring a user model of its own.

use axum::{
    extract::{FromRequestParts, OptionalFromRequestParts},
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
};
use layer2_core::{Authorization, Caller, SubjectId};

use crate::rest::{problem_document, problem_response};

/// The authenticated principal for the current request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedSubject(pub SubjectId);

impl AuthenticatedSubject {
    /// Borrow the underlying subject.
    pub fn subject(&self) -> &SubjectId {
        &self.0
    }
}

/// The subject whose work a request reached, when it was not the caller's own.
///
/// Placed on the *response* by the handler that established it, for whatever
/// records requests to read back. Anything sitting outside the handlers sees a
/// method and a path and nothing else, and at that remove "an operator ended
/// somebody's session" and "somebody ended their own" are the same request to
/// the same shape of address — so the one fact that distinguishes them has to
/// be carried out from where it is known.
///
/// Absent means the caller was acting on their own work, which needs no
/// second name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActedUpon(pub SubjectId);

/// What a platform-settings write actually changed.
///
/// Placed on the response by the handler, where the stored before and after
/// documents are both known. Middleware outside the handler deliberately does
/// not inspect the request body because it may contain administrator-authored
/// prose.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SettingsTouched {
    /// Setting names from the core document, in declaration order.
    pub changed: Vec<&'static str>,
    /// The sealed card appended by this write, when pricing changed.
    pub rate_card: Option<String>,
}

/// The authenticated caller: who they are and what they may do.
///
/// Extracted as one value so a handler cannot accidentally pass identity
/// without the capabilities that qualify it. A request that authenticated but
/// carries no resolvable capabilities yields an empty authorization, which
/// denies everything rather than defaulting open.
#[derive(Debug, Clone)]
pub struct AuthenticatedCaller(pub Caller);

impl<S> FromRequestParts<S> for AuthenticatedCaller
where
    S: Send + Sync,
{
    type Rejection = NotAuthenticated;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let subject = parts
            .extensions
            .get::<AuthenticatedSubject>()
            .cloned()
            .ok_or(NotAuthenticated)?;
        let authorization = parts
            .extensions
            .get::<Authorization>()
            .cloned()
            .unwrap_or_else(Authorization::none);
        Ok(Self(Caller::new(subject.0, authorization)))
    }
}

/// Absent rather than refused, for the one route that has a second way in.
///
/// The terminal upgrade accepts either an already-authenticated caller or a
/// ticket, and it must be able to look at both before deciding. Every other
/// route asks for [`AuthenticatedCaller`] directly and is refused without one.
impl<S> OptionalFromRequestParts<S> for AuthenticatedCaller
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(
            <Self as FromRequestParts<S>>::from_request_parts(parts, state)
                .await
                .ok(),
        )
    }
}

/// A credential the authenticated subject has delegated for this request.
///
/// Separate from the subject on purpose. Identity is needed by every handler;
/// a delegated credential is needed by almost none, so extracting it has to be
/// a deliberate act rather than something that rides along by default.
#[derive(Clone)]
pub struct DelegatedCredential {
    value: String,
    request_bound: bool,
}

impl DelegatedCredential {
    /// A browser credential that follows the subject's renewable state.
    pub fn subject(value: String) -> Self {
        Self {
            value,
            request_bound: false,
        }
    }

    /// A bearer credential whose exact authority belongs to this request.
    pub fn request_bound(value: String) -> Self {
        Self {
            value,
            request_bound: true,
        }
    }

    /// Borrow the opaque value only where it must be delegated onward.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Consume the provenance wrapper where only the opaque value is needed.
    pub fn into_value(self) -> String {
        self.value
    }

    /// Carry both the value and its recovery policy into session creation.
    pub fn into_caller_credential(self) -> layer2_core::CallerCredential {
        if self.request_bound {
            layer2_core::CallerCredential::request_bound(self.value)
        } else {
            self.value.into()
        }
    }

    /// Whether this exact value must remain pinned across session recovery.
    pub fn is_request_bound(&self) -> bool {
        self.request_bound
    }
}

impl std::fmt::Debug for DelegatedCredential {
    /// Redacted, so no accidental `{:?}` puts a live credential in a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DelegatedCredential(<redacted {} bytes>, request_bound={})",
            self.value.len(),
            self.request_bound
        )
    }
}

/// Rejection returned when no verified subject is present.
#[derive(Debug)]
pub struct NotAuthenticated;

impl IntoResponse for NotAuthenticated {
    fn into_response(self) -> Response {
        let status = StatusCode::UNAUTHORIZED;
        let mut body = problem_document(
            status,
            "not-authenticated",
            "This operation requires an authenticated subject.",
            "not_authenticated",
        );
        body["title"] = "Not authenticated".into();
        problem_response(status, body)
    }
}

/// The service could not establish identity because its durable store failed.
///
/// Deliberately distinct from [`NotAuthenticated`]: retrying a bad credential
/// does not help, while retrying after a store outage is exactly the remedy.
#[derive(Debug)]
pub struct StoreUnavailable;

impl IntoResponse for StoreUnavailable {
    fn into_response(self) -> Response {
        let status = StatusCode::SERVICE_UNAVAILABLE;
        let mut body = problem_document(
            status,
            "store-failed",
            "The service could not read or write its durable authentication state.",
            "store_failed",
        );
        body["title"] = "Store unavailable".into();
        problem_response(status, body)
    }
}

impl<S> FromRequestParts<S> for AuthenticatedSubject
where
    S: Send + Sync,
{
    type Rejection = NotAuthenticated;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthenticatedSubject>()
            .cloned()
            .ok_or(NotAuthenticated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    #[test]
    fn delegated_credential_conversion_keeps_request_provenance_attached() {
        let request = DelegatedCredential::request_bound("bearer".into()).into_caller_credential();
        assert!(request.is_request_bound());
        assert_eq!(request.as_ref(), "bearer");

        let subject = DelegatedCredential::subject("browser".into()).into_caller_credential();
        assert!(!subject.is_request_bound());
        assert_eq!(subject.as_ref(), "browser");
    }

    #[tokio::test]
    async fn a_subject_placed_by_middleware_is_extracted() {
        let mut req = Request::builder().body(()).expect("request builds");
        req.extensions_mut()
            .insert(AuthenticatedSubject(SubjectId("subject-a".into())));
        let (mut parts, _) = req.into_parts();

        let got = AuthenticatedSubject::from_request_parts(&mut parts, &())
            .await
            .expect("subject is present");
        assert_eq!(got.subject().as_str(), "subject-a");
    }

    #[tokio::test]
    async fn a_request_without_a_subject_is_rejected() {
        let req = Request::builder().body(()).expect("request builds");
        let (mut parts, _) = req.into_parts();
        let result = AuthenticatedSubject::from_request_parts(&mut parts, &()).await;
        assert!(result.is_err(), "unauthenticated requests must be rejected");
    }

    #[tokio::test]
    async fn rejection_renders_as_a_coded_problem_document_with_401() {
        async fn protected(_subject: AuthenticatedSubject) {}

        let response = Router::new()
            .route("/protected", get(protected))
            .oneshot(
                Request::get("/protected")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static(
                "application/problem+json"
            ))
        );
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("problem documents are small"),
        )
        .expect("the problem document is json");
        assert_eq!(body["code"], "not_authenticated");
    }

    #[tokio::test]
    async fn a_store_outage_is_a_retryable_typed_503() {
        let response = StoreUnavailable.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static(
                "application/problem+json"
            ))
        );
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("problem documents are small"),
        )
        .expect("the problem document is json");
        assert_eq!(body["status"], 503);
        assert_eq!(body["code"], "store_failed");
        assert!(body["type"].as_str().unwrap().ends_with("/store-failed"));
    }
}
