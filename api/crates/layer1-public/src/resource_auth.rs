use axum::{
    extract::{FromRequestParts, OptionalFromRequestParts},
    http::request::Parts,
};
use layer2_core::resource_access::CallerContext;

use crate::{AuthenticatedCaller, DelegatedCredential};

/// Nonsecret verified identity plus the optional request credential, with the
/// same redaction and zeroization as existing outbound credential handling.
#[derive(Debug, Clone)]
pub struct AuthenticatedResourceCaller {
    pub context: CallerContext,
    pub credential: Option<DelegatedCredential>,
}

impl<S: Send + Sync> FromRequestParts<S> for AuthenticatedResourceCaller {
    type Rejection = crate::auth::NotAuthenticated;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let AuthenticatedCaller(caller) =
            <AuthenticatedCaller as FromRequestParts<S>>::from_request_parts(parts, state).await?;
        let context = parts
            .extensions
            .get::<CallerContext>()
            .cloned()
            .filter(|context| context.validate_principal() && context.caller == caller)
            .ok_or(crate::auth::NotAuthenticated)?;
        Ok(Self {
            context,
            credential: parts.extensions.get::<DelegatedCredential>().cloned(),
        })
    }
}

// Absence supports older service compositions. Invalid present context is an
// authentication error and must never silently fall back to legacy authority.
impl<S: Send + Sync> OptionalFromRequestParts<S> for AuthenticatedResourceCaller {
    type Rejection = crate::auth::NotAuthenticated;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        if parts.extensions.get::<CallerContext>().is_none() {
            return Ok(None);
        }
        <Self as FromRequestParts<S>>::from_request_parts(parts, state)
            .await
            .map(Some)
    }
}
