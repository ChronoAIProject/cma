//! Attach verified identity and authority to one incoming request.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use layer1_public::{AuthenticatedSubject, DelegatedCredential};

use super::AuthState;
use layer2_core::dependency_diagnostics::{
    self as diagnostics, Dependency, Failure, Operation, Reason,
};

mod failure_diagnostics;

pub async fn attach_subject(
    state: State<Arc<AuthState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let account_route = request.uri().path() == "/api/v1/account/deletion"
        && matches!(request.method().as_str(), "GET" | "POST");
    let mut response = attach_subject_inner(state, request, next).await;
    if account_route {
        response.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("private, no-store"),
        );
    }
    response
}

async fn attach_subject_inner(
    State(auth): State<Arc<AuthState>>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    if request.uri().path() == layer1_public::rest::creation_outcomes::LOOKUP_PATH {
        if !auth.creation_outcomes_enabled {
            return StatusCode::NOT_FOUND.into_response();
        }
        return super::creation_outcomes::attach(&auth, request, next).await;
    }
    if super::worker_credentials::matches_path(request.uri().path()) {
        return super::worker_credentials::attach(&auth, request, next).await;
    }
    if request
        .headers()
        .contains_key(super::resource_request::HEADER)
    {
        return super::resource_request::attach(&auth, request, next).await;
    }
    if bypasses_ambient_auth(request.uri().path()) {
        let mut observation =
            crate::request_observation::Resolution::start(&request, "independent");
        observation.finish("bypassed", crate::request_observation::Consumer::Unknown);
        drop(observation);
        return next.run(request).await;
    }
    let mut observation = crate::request_observation::Resolution::start(&request, "ambient");
    let ((resolved, failure), retry_after) = super::identity_throttle::capture(
        layer2_core::diagnostic_context::DiagnosticContext::current()
            .with_phase("identity")
            .scope(diagnostics::capture(super::identity_lifetime::scope(
                &auth,
                auth.resolve_request(request.headers()),
                super::identity_retry::unavailable,
            ))),
    )
    .await;
    let outcome = match &resolved {
        Ok(Some(_)) => "resolved",
        Ok(None) => "anonymous",
        Err(_) => "unavailable",
    };
    // The resolver exclusively verifies this path when configured; an accepted
    // proxy read need not have enrolled renewable owner authority.
    let verified_proxy =
        auth.identity_proxy.is_some() && request.headers().contains_key(super::nyxid_proxy::HEADER);
    let consumer = resolved.as_ref().ok().and_then(Option::as_ref).map_or(
        crate::request_observation::Consumer::Unknown,
        |request| match request.credential_source {
            _ if verified_proxy => crate::request_observation::Consumer::Nyxid,
            layer2_core::resource_access::CredentialSource::IdentityProxy { .. } => {
                crate::request_observation::Consumer::Nyxid
            }
            layer2_core::resource_access::CredentialSource::Independent { .. }
            | layer2_core::resource_access::CredentialSource::Binding => {
                crate::request_observation::Consumer::DirectApi
            }
            layer2_core::resource_access::CredentialSource::Retained
                if matches!(auth.mode, super::AuthMode::Provider { .. }) =>
            {
                crate::request_observation::Consumer::PublicUser
            }
            _ => crate::request_observation::Consumer::Unknown,
        },
    );
    observation.finish(outcome, consumer);
    drop(observation);
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(error) => {
            let failure = failure.unwrap_or(Failure::new(
                Dependency::AuthenticationAuthority,
                Reason::Unknown,
            ));
            failure_diagnostics::record(&error, failure);
            diagnostics::report(Operation::Authentication, Some(failure));
            if let Some(seconds) = retry_after {
                return identity_failure_after(seconds);
            }
            if (request.method() == axum::http::Method::GET
                || request.uri().path() == "/auth/session")
                && matches!(&error, layer2_core::StoreError::Unavailable(detail) if detail == "identity_provider_unavailable")
            {
                return identity_failure();
            }
            let mut response = store_failure();
            if request.uri().path().starts_with("/auth/") {
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    "no-store".parse().unwrap(),
                );
            }
            return response;
        }
    };
    if let Some(resolved) = resolved {
        let owner_session = auth.owner_session_authority(&resolved);
        if let Some(response) = auth
            .account_gate(
                request.method().as_str(),
                request.uri().path(),
                &resolved.subject,
                owner_session.is_some(),
            )
            .await
        {
            return response;
        }
        if request.uri().path() == "/api/v1/bots"
            || request.uri().path().starts_with("/api/v1/bots/")
        {
            match auth.authorize_bot_request(&resolved).await {
                Ok(true) => {},
                Ok(false) => return (
                    StatusCode::FORBIDDEN,
                    [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
                    Json(serde_json::json!({"type":"about:blank","title":"renewable owner authority required","status":403,
                        "detail":"Sign in to CMA with this account to authorize renewable Bot credentials.","code":"bot_owner_authority_required"})),
                ).into_response(),
                Err(_) => return store_failure(),
            }
        }
        if let Some(binding) = &resolved.binding {
            if !super::binding::permits_request(
                request.method(),
                request.uri().path(),
                binding.binding_id(),
            ) {
                return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error":{"code":"binding_key_scope","message":"Binding keys are limited to their conversation interface.","type":"permission_error"}}))).into_response();
            }
            request
                .extensions_mut()
                .insert(layer1_public::AuthMethod::BindingKey {
                    binding_id: binding.binding_id().into(),
                });
        }
        if request.uri().path().starts_with("/api/v1/agents/")
            && request.uri().path().ends_with("/desktop/ws")
        {
            // The domain service uses this independent port only in replica
            // mode. Retain legacy authentication behavior if capture is refused.
            match super::desktop::capture(auth.clone(), request.headers(), &resolved) {
                Ok(authority) => {
                    request.extensions_mut().insert(authority);
                }
                Err(_) => tracing::debug!("desktop renewable authority unavailable"),
            }
        }
        request
            .extensions_mut()
            .insert(auth.resource_request_context(&resolved));
        if let Some(authority) = owner_session {
            request.extensions_mut().insert(authority);
        }
        if let Some(value) = resolved.delegated_credential {
            request
                .extensions_mut()
                .insert(DelegatedCredential::new(value));
        }
        request.extensions_mut().insert(resolved.authorization);
        request.extensions_mut().insert(resolved.profile);
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(resolved.subject));
    }
    next.run(request).await
}

pub fn identity_failure() -> Response {
    identity_failure_after(3)
}

fn identity_failure_after(seconds: u64) -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        [
            (axum::http::header::CONTENT_TYPE, "application/problem+json"),
            (axum::http::header::RETRY_AFTER, "3"),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        Json(serde_json::json!({
            "type": "https://spec.chrono-ai.fun/cma/1.0/problems/identity-unavailable",
            "title": "Identity service temporarily unavailable",
            "status": 503,
            "detail": "The identity service is temporarily unavailable. Please retry shortly.",
            "code": "identity_unavailable",
            "retryable": true,
        })),
    )
        .into_response();
    response.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        seconds.max(1).to_string().parse().expect("integer header"),
    );
    response
}

pub fn store_failure() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "type": "https://spec.chrono-ai.fun/cma/1.0/problems/store-failed",
            "title": "store failed",
            "status": 503,
            "detail": "Authentication authority is temporarily unavailable.",
            "code": "store_failed",
            // Invalid provider evidence also reaches this response. A 503 alone
            // must not authorize automatic browser identity retries.
            "retryable": false,
        })),
    )
        .into_response()
}

fn bypasses_ambient_auth(path: &str) -> bool {
    // Recovery routes validate their own authority without requiring an old login.
    if matches!(
        path,
        "/auth/signup-options"
            | "/auth/login"
            | "/auth/callback"
            | super::service_authorization::CALLBACK_PATH
            | "/auth/logout"
            | "/.well-known/agent-card.json"
    ) {
        return true;
    }
    let Some(path) = path.strip_prefix('/') else {
        return false;
    };
    let entry = path.split('/').collect::<Vec<_>>();
    if matches!(entry.as_slice(),
        ["api", "v1", "triggers", "public", public_id, "entry-intent"]
            if !public_id.is_empty())
        || matches!(entry.as_slice(),
            ["api", "v1", "triggers", "public", public_id, "open", placement_id]
                if !public_id.is_empty() && !placement_id.is_empty())
    {
        return true;
    }
    let mut segments = path.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ),
        (
            Some("api"),
            Some("v1"),
            Some("triggers"),
            Some("public"),
            Some(public_id),
            None,
            None,
        ) | (
            Some("api"),
            Some("v1"),
            Some("triggers"),
            Some("public"),
            Some(public_id),
            Some("badge.svg"),
            None,
        ) if !public_id.is_empty()
    )
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "error_metadata_tests.rs"]
mod error_metadata_tests;
