//! Provider-authorized, exact-request reads. This credential has no durable custody.

use std::{sync::Arc, time::SystemTime};

use axum::{
    body::{to_bytes, Body},
    extract::{OriginalUri, Request},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use layer1_public::AuthenticatedSubject;
use layer2_core::{
    resource_access::{CallerContext, CredentialContext, CredentialSource, ResourceActor},
    Caller,
};

use super::{profile::Profile, AuthMode, AuthState};

mod descriptor;
mod verification;

#[cfg(feature = "test-support")]
pub fn request_descriptor(
    method: &axum::http::Method,
    uri: &axum::http::Uri,
    headers: &axum::http::HeaderMap,
) -> Option<impl serde::Serialize> {
    descriptor::Descriptor::from_request(method, uri, headers).ok()
}

pub const HEADER: &str = "x-nyxid-resource-request";

impl AuthState {
    pub fn with_resource_request_enabled(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("request proof policy is configured before sharing auth")
            .resource_request_enabled = enabled;
        self
    }
}

pub(super) async fn attach(auth: &AuthState, request: Request, next: Next) -> Response {
    use crate::request_observation::{Consumer, Resolution};
    let mut observation = Resolution::start(&request, "resource_request");
    if !auth.resource_request_enabled {
        return observation.rejected(refusal(StatusCode::FORBIDDEN, "resource_request_disabled"));
    }
    let (mut parts, body) = request.into_parts();
    let uri = parts
        .extensions
        .get::<OriginalUri>()
        .map_or(&parts.uri, |original| &original.0);
    let descriptor = match descriptor::Descriptor::from_request(&parts.method, uri, &parts.headers)
    {
        Ok(value) => value,
        Err(()) => {
            return observation.rejected(refusal(
                StatusCode::FORBIDDEN,
                "resource_request_unsupported",
            ))
        }
    };
    let proof = match descriptor::one_header(&parts.headers, HEADER) {
        Ok(Some(value))
            if !value.is_empty() && value.len() <= 16 * 1024 && value.trim() == value =>
        {
            zeroize::Zeroizing::new(value.to_owned())
        }
        _ => {
            return observation.rejected(refusal(
                StatusCode::UNAUTHORIZED,
                "resource_request_invalid",
            ))
        }
    };
    // v1 permits zero body bytes. Bound even a streaming empty-body sender so
    // this authentication path cannot retain a connection indefinitely.
    match tokio::time::timeout(std::time::Duration::from_secs(2), to_bytes(body, 0)).await {
        Ok(Ok(bytes)) if bytes.is_empty() => {}
        _ => {
            return observation.rejected(refusal(
                StatusCode::FORBIDDEN,
                "resource_request_unsupported",
            ))
        }
    }
    let verified = match verification::verify(auth, &proof, &descriptor).await {
        Ok(value) => value,
        Err(verification::Error::Rejected) => {
            return observation.rejected(refusal(
                StatusCode::UNAUTHORIZED,
                "resource_request_invalid",
            ))
        }
        Err(verification::Error::Unavailable) => {
            observation.finish("unavailable", Consumer::Unknown);
            return refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_request_unavailable",
            );
        }
    };
    let AuthMode::Provider { base_url, .. } = &auth.mode else {
        return observation.rejected(refusal(
            StatusCode::UNAUTHORIZED,
            "resource_request_invalid",
        ));
    };
    if let Some(response) = auth
        .account_gate(
            parts.method.as_str(),
            parts.uri.path(),
            &verified.subject,
            false,
        )
        .await
    {
        return observation.rejected(response);
    }
    let context = CallerContext {
        credential_source: CredentialSource::RequestBoundRead {
            expires_at: verified.expires_at,
        },
        caller: Caller::new(verified.subject.clone(), verified.authorization.clone()),
        credential: CredentialContext {
            issuer: base_url.trim_end_matches('/').into(),
            subject: verified.subject.clone(),
            credential_id: super::persistence::SecretDigest::identify(
                "resource-request-read",
                &proof,
            )
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        },
        actor: ResourceActor::Api {
            subject: verified.subject.clone(),
            delegation_id: None,
        },
        memberships: Vec::new(),
    };
    let Some(deadline) = context.credential_deadline() else {
        return observation.rejected(refusal(
            StatusCode::UNAUTHORIZED,
            "resource_request_invalid",
        ));
    };
    // The proof wins over ambient credentials. None of them reach a public
    // handler or become an outbound/deferred credential on this path.
    parts.headers.remove(HEADER);
    parts.headers.remove(header::AUTHORIZATION);
    parts.headers.remove(header::COOKIE);
    if auth.proxy_require_request_proof {
        parts.headers.remove(super::nyxid_proxy::HEADER);
    }
    parts
        .extensions
        .remove::<layer1_public::DelegatedCredential>();
    parts.extensions.remove::<layer1_public::AuthMethod>();
    parts.extensions.insert(context);
    parts.extensions.insert(verified.authorization);
    parts.extensions.insert(Profile::default());
    parts
        .extensions
        .insert(AuthenticatedSubject(verified.subject));
    observation.finish("resolved", Consumer::Nyxid);
    drop(observation);
    match tokio::time::timeout_at(
        deadline,
        next.run(Request::from_parts(parts, Body::empty())),
    )
    .await
    {
        Ok(response) if SystemTime::now() < verified.expires_at => response,
        _ => refusal(StatusCode::UNAUTHORIZED, "resource_request_expired"),
    }
}

fn refusal(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/problem+json")],
        Json(serde_json::json!({
            "type": "about:blank", "title": "Request authorization unavailable",
            "status": status.as_u16(), "code": code,
            "detail": "This credential cannot authorize the requested read.",
        })),
    )
        .into_response()
}
