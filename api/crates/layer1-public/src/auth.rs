//! Extract authenticated identity and delegated credentials at public boundaries.

use axum::{
    extract::{FromRequestParts, OptionalFromRequestParts},
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
};
use layer2_core::{Authorization, Caller, SubjectId};
use zeroize::Zeroize;

use crate::rest::{problem_document, problem_response};

/// The authenticated principal for the current request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedSubject(pub SubjectId);

impl AuthenticatedSubject {
    pub fn subject(&self) -> &SubjectId {
        &self.0
    }
}

/// The authenticated caller and authority resolved by middleware.
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

/// A credential delegated for exactly one outbound request.
#[derive(Clone)]
pub struct DelegatedCredential(String);

impl DelegatedCredential {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn value(&self) -> &str {
        &self.0
    }

    pub fn into_value(mut self) -> String {
        std::mem::take(&mut self.0)
    }
}

impl std::fmt::Debug for DelegatedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DelegatedCredential(<redacted {} bytes>)", self.0.len())
    }
}

impl Drop for DelegatedCredential {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Rejection returned when no verified subject is present.
#[derive(Debug)]
pub struct NotAuthenticated;

impl IntoResponse for NotAuthenticated {
    fn into_response(self) -> Response {
        let status = StatusCode::UNAUTHORIZED;
        problem_response(
            status,
            problem_document(
                status,
                "not-authenticated",
                "This operation requires an authenticated subject.",
                "not_authenticated",
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn middleware_identity_is_extracted_with_default_deny_authority() {
        let mut request = axum::http::Request::builder().body(()).unwrap();
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(SubjectId::new("subject-a")));
        let (mut parts, _) = request.into_parts();

        let caller =
            <AuthenticatedCaller as FromRequestParts<()>>::from_request_parts(&mut parts, &())
                .await
                .unwrap();
        assert_eq!(caller.0.subject.as_str(), "subject-a");
        assert_eq!(caller.0.authorization, Authorization::none());
    }

    #[test]
    fn delegated_credentials_are_redacted() {
        let credential = DelegatedCredential::new("bearer-secret".into());
        assert!(!format!("{credential:?}").contains("bearer-secret"));
        assert_eq!(credential.value(), "bearer-secret");
    }
}
