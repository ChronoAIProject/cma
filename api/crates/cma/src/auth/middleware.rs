//! Putting the authenticated subject on the request.
//!
//! Split from the state so that file is about identity and this one about the
//! single place identity meets a request. Everything downstream reads what this
//! attaches and never a header, which is what stops an interface from acquiring
//! a user model of its own.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderMap},
    response::Response,
};

use layer1_public::AuthenticatedSubject;

use super::{AuthState, SESSION_COOKIE};

pub(crate) fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(k, _)| *k == SESSION_COOKIE)
        .map(|(_, v)| v.to_string())
}

/// Middleware attaching the authenticated subject to the request.
pub async fn attach_subject(
    State(auth): State<Arc<AuthState>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if let Some(subject) = auth.resolve(req.headers()).await {
        // The credential rides separately from identity: nearly every handler
        // needs the subject, almost none needs a credential, so taking one has
        // to be deliberate.
        if let Some(token) = auth.access_token(&subject).await {
            req.extensions_mut()
                .insert(layer1_public::auth::DelegatedCredential(token));
        }
        // The same resolution the identity endpoint reports, so what a client
        // was told it may do and what this request is allowed to do cannot
        // drift apart.
        req.extensions_mut()
            .insert(auth.effective_authorization(&subject).await);
        req.extensions_mut().insert(AuthenticatedSubject(subject));
    }
    next.run(req).await
}
