//! Complete provider permission evidence, preferring advertised introspection.

use super::verification_diagnostics::{claims, configuration, Endpoint};
use super::verification_error::ProviderVerificationError;
use super::{AuthMode, AuthState, ProviderMetadata};
use std::time::{Duration, SystemTime};

pub(super) struct PermissionObservation {
    pub permissions: PermissionEvidence,
    pub api_expires_at: Option<SystemTime>,
    pub token_expires_at: Option<SystemTime>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PermissionEvidence {
    Active(Vec<String>),
    Inactive,
}

pub(super) fn permissions_claim(body: &serde_json::Value) -> Result<Vec<String>, String> {
    let permissions = body
        .get("permissions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "provider response has no complete permissions array".to_string())?;
    permissions
        .iter()
        .map(|permission| {
            permission
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "provider permissions array contains a non-string value".to_string())
        })
        .collect()
}

fn introspection_claim(
    body: &serde_json::Value,
    expected_subject: &str,
) -> Result<PermissionEvidence, ProviderVerificationError> {
    use ProviderVerificationError::{Rejected, Unavailable};
    let active = body
        .get("active")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            Unavailable("introspection response has no boolean active disposition".into())
        })?;
    if let Some(subject) = body.get("sub") {
        if subject.as_str() != Some(expected_subject) {
            return Err(Rejected(
                "introspection subject does not match userinfo".into(),
            ));
        }
    }
    if !active {
        return Ok(PermissionEvidence::Inactive);
    }
    permissions_claim(body)
        .map(PermissionEvidence::Active)
        .map_err(Unavailable)
}

/// Unvalidated provider response, bound to userinfo only after both reads finish.
pub(super) struct IntrospectionResponse(serde_json::Value);

/// What the provider says this token may do, asked as this service.
///
/// `None` when the provider advertises no introspection endpoint, which is the
/// signal to fall back to the userinfo claim rather than to deny.
///
/// Inactive evidence is distinct from an active token with no permissions: only
/// the latter may remain a delegated credential.
pub(super) async fn fetch_introspection(
    auth: &AuthState,
    meta: &ProviderMetadata,
    access_token: &str,
    deadline: tokio::time::Instant,
) -> Result<Option<IntrospectionResponse>, ProviderVerificationError> {
    use ProviderVerificationError::Unavailable;
    let Some(endpoint) = meta.introspection_endpoint.as_ref() else {
        return Ok(None);
    };
    let AuthMode::Provider {
        client_id,
        client_secret,
        ..
    } = &auth.mode
    else {
        return configuration(
            Endpoint::Introspection,
            Err(Unavailable(
                "introspection requires provider client credentials".into(),
            )),
        );
    };

    let form = [
        ("token", access_token),
        ("token_type_hint", "access_token"),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
    ];
    // The form carries the secret, so nothing here logs it.
    let body = super::identity_cooldown::json(
        auth,
        access_token,
        endpoint,
        super::verification_diagnostics::Endpoint::Introspection,
        auth.http.post(endpoint).form(&form),
        deadline,
    )
    .await?;
    claims(
        Endpoint::Introspection,
        (|| {
            if body.get("active").and_then(serde_json::Value::as_bool) == Some(false) {
                return Err(ProviderVerificationError::Rejected(
                    "provider reports an inactive token".into(),
                ));
            }
            // Partial/malformed success cannot be masked by the other endpoint's outage.
            if body.get("active").and_then(serde_json::Value::as_bool) != Some(true) {
                return Err(Unavailable(
                    "introspection response has no boolean active disposition".into(),
                ));
            }
            permissions_claim(&body).map_err(Unavailable)?;
            token_expiry(&body)?;
            Ok(Some(IntrospectionResponse(body)))
        })(),
    )
}

impl IntrospectionResponse {
    pub(super) fn require_subject(&self, expected: &str) -> Result<(), ProviderVerificationError> {
        if self
            .0
            .get("sub")
            .is_some_and(|value| value.as_str() != Some(expected))
        {
            return Err(ProviderVerificationError::Rejected(
                "introspection subject does not match retained owner".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn verify(
        self,
        expected_subject: &str,
        resource: Option<&str>,
        resource_required: bool,
    ) -> Result<PermissionObservation, ProviderVerificationError> {
        claims(
            Endpoint::Introspection,
            (|| {
                let body = self.0;
                let token_expires_at = token_expiry(&body)?;
                introspection_claim(&body, expected_subject).map(|permissions| {
                    let api_expires_at = (body.get("active").and_then(serde_json::Value::as_bool)
                        == Some(true)
                        && body.get("sub").and_then(serde_json::Value::as_str)
                            == Some(expected_subject)
                        && body.get("token_type").and_then(serde_json::Value::as_str)
                            == Some("access")
                        && super::oauth_commands::provider_allows(
                            &body,
                            resource,
                            resource_required,
                        ))
                    .then(|| body.get("exp").and_then(serde_json::Value::as_u64))
                    .flatten()
                    .and_then(|seconds| {
                        SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
                    })
                    .filter(|expiry| *expiry > SystemTime::now());
                    PermissionObservation {
                        permissions,
                        api_expires_at,
                        token_expires_at,
                    }
                })
            })(),
        )
    }
}

pub(super) fn token_expiry(
    body: &serde_json::Value,
) -> Result<Option<SystemTime>, ProviderVerificationError> {
    body.get("exp")
        .map(|value| {
            value
                .as_u64()
                .and_then(|seconds| {
                    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
                })
                .filter(|expiry| *expiry > SystemTime::now())
                .ok_or_else(|| {
                    ProviderVerificationError::Rejected("invalid or expired token lifetime".into())
                })
        })
        .transpose()
}

#[cfg(test)]
#[path = "permissions_tests.rs"]
mod tests;
