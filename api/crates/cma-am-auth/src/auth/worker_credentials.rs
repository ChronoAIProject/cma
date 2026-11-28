//! Exact service-only authentication for the Gateway worker-key exclusion check.
use super::{bearer, identity, permissions::PermissionEvidence, AuthMode, AuthState};
use axum::{
    extract::Request,
    http::{header, Method, StatusCode},
    middleware::Next,
    response::Response,
};
use base64::Engine as _;
use layer1_public::rest::worker_credentials::{
    failure, VerifiedGatewayService, WORKER_CLASSIFIER_PATH,
};
use layer2_core::{Capability, SubjectId};
use serde_json::Value;
use std::time::Duration;

pub fn matches_path(path: &str) -> bool {
    matches!(
        path,
        WORKER_CLASSIFIER_PATH | cmaeg_core::request_audit::PATH
    )
}

#[derive(Clone)]
pub struct VerifiedAuditWriter(pub(crate) SubjectId);

impl VerifiedAuditWriter {
    pub fn subject(&self) -> &SubjectId {
        &self.0
    }
}

pub(super) async fn attach(auth: &AuthState, mut request: Request, next: Next) -> Response {
    use crate::request_observation::{Consumer, Resolution};
    let mut observation = Resolution::start(&request, "worker");
    if request.method() != Method::POST
        || request.uri().query().is_some()
        || request.headers().contains_key(header::COOKIE)
        || request
            .headers()
            .contains_key(super::resource_request::HEADER)
        || request.headers().contains_key(super::nyxid_proxy::HEADER)
    {
        return observation.rejected(rejected());
    }
    let Some(token) = bearer::bearer_token(request.headers()) else {
        return observation.rejected(rejected());
    };
    let Some(subject) = service_subject(&token) else {
        return observation.rejected(rejected());
    };
    let audit = request.uri().path() == cmaeg_core::request_audit::PATH;
    match tokio::time::timeout(
        Duration::from_secs(2),
        verify(auth, &token, &subject, audit),
    )
    .await
    {
        Ok(Ok(Some(subject))) => {
            if let Some(response) = auth
                .account_gate(
                    request.method().as_str(),
                    request.uri().path(),
                    &subject,
                    false,
                )
                .await
            {
                return observation.rejected(response);
            }
            observation.finish("resolved", Consumer::Internal);
            drop(observation);
            request
                .extensions_mut()
                .insert(layer1_public::AuthenticatedSubject(subject.clone()));
            if audit {
                request
                    .extensions_mut()
                    .insert(VerifiedAuditWriter(subject));
            } else {
                request
                    .extensions_mut()
                    .insert(VerifiedGatewayService::new(subject));
            }
            next.run(request).await
        }
        Ok(Ok(None)) => observation.rejected(failure(
            StatusCode::FORBIDDEN,
            "worker_classifier_forbidden",
            "Current Gateway machine service authority is required.",
        )),
        _ => {
            observation.finish("unavailable", Consumer::Unknown);
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "worker_classifier_unavailable",
                "Gateway machine service authority is temporarily unavailable.",
            )
        }
    }
}

fn rejected() -> Response {
    failure(
        StatusCode::UNAUTHORIZED,
        "worker_classifier_auth_required",
        "Gateway machine service authentication is required.",
    )
}

async fn verify(
    auth: &AuthState,
    token: &str,
    subject: &str,
    audit: bool,
) -> Result<Option<SubjectId>, ()> {
    if !matches!(auth.mode, AuthMode::Provider { .. }) {
        return Err(());
    }
    let metadata = auth.metadata().await.map_err(|_| ())?;
    // Userinfo alone cannot establish that the exact service JWT was verified.
    if metadata.introspection_endpoint.is_none() {
        return Err(());
    }
    let verified = match identity::verify_subject(auth, &metadata, token).await {
        Ok(value) => value,
        Err(super::verification_error::ProviderVerificationError::Rejected(_)) => return Ok(None),
        Err(_) => return Err(()),
    };
    if verified.subject != SubjectId::new(subject) || verified.api_expires_at.is_none() {
        return Ok(None);
    }
    let PermissionEvidence::Active(names) = verified.permissions else {
        return Ok(None);
    };
    if audit
        && !names
            .iter()
            .any(|name| name == cmaeg_core::request_audit::WRITE_PERMISSION)
    {
        return Ok(None);
    }
    let grants = crate::permissions::capabilities_from_permissions(names);
    Ok((grants.allows(Capability::ManageProviderBindings)
        && grants.allows(Capability::ManageGatewayRecords))
    .then_some(verified.subject))
}

/// This only refuses unsuitable shapes. The exact original bearer must then
/// pass current provider userinfo AND introspection, binding its signed subject,
/// access-token type, expiry and permissions before the marker is installed.
fn service_subject(token: &str) -> Option<String> {
    if token.len() > 16 * 1024 || !bearer::permits_ambient_auth(token) {
        return None;
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    if claims.get("sa").and_then(Value::as_bool) != Some(true)
        || claims.get("token_type").and_then(Value::as_str) != Some("access")
        || claims
            .get("delegated")
            .is_some_and(|value| !value.is_null() && value != &Value::Bool(false))
        || [
            "act",
            "cnf",
            "resources",
            "resource",
            "api_key_id",
            "allowed_service_ids",
            "allowed_node_ids",
            "allow_all_services",
            "allow_all_nodes",
            "assistant_forward",
        ]
        .iter()
        .any(|field| claims.get(*field).is_some_and(|value| !value.is_null()))
    {
        return None;
    }
    claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|subject| !subject.is_empty() && subject.len() <= 256 && subject.trim() == *subject)
        .map(str::to_owned)
}
