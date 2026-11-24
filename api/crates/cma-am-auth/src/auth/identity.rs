//! Provider verification for credentials CMA did not issue.

use layer2_core::StoreError;

use super::verification_diagnostics::{claims, configuration, Endpoint};
use super::verification_error::ProviderVerificationError;
use super::{permissions, profile::Profile, AuthState, ProviderMetadata, RequestAuth};

pub struct VerifiedSubject {
    pub subject: layer2_core::SubjectId,
    pub(super) permissions: permissions::PermissionEvidence,
    pub profile: Profile,
    pub independent_commands: bool,
    pub api_expires_at: Option<std::time::SystemTime>,
    pub token_expires_at: Option<std::time::SystemTime>,
    pub(super) verification: Option<super::identity_lifetime::VerificationLifetime>,
}

pub(super) fn verification_failure(stage: &'static str, reason: &'static str, status: Option<u16>) {
    tracing::warn!(
        stage,
        reason,
        status,
        "identity provider verification failed"
    );
}

pub async fn request_for_bearer(
    auth: &AuthState,
    token: String,
) -> Result<Option<RequestAuth>, StoreError> {
    match super::bearer_authorization::verify_request(auth, &token).await {
        Ok(verified) => match &verified.permissions {
            permissions::PermissionEvidence::Active(permission_names) => Ok(Some(RequestAuth {
                binding: None,
                credential_source: layer2_core::resource_access::CredentialSource::Independent {
                    expires_at: verified.api_expires_at,
                },
                subject: verified.subject.clone(),
                authorization: crate::permissions::capabilities_from_permissions(
                    permission_names.iter().cloned(),
                ),
                profile: verified.profile.clone(),
                delegated_credential: Some(token),
            })),
            permissions::PermissionEvidence::Inactive => Ok(None),
        },
        Err(ProviderVerificationError::Rejected(error)) => {
            tracing::debug!(error = %error, "bearer credential was not accepted");
            Ok(None)
        }
        Err(
            error @ (ProviderVerificationError::Transient(_)
            | ProviderVerificationError::RateLimited(_)),
        ) => {
            error.record_unavailable();
            Err(super::identity_retry::unavailable())
        }
        Err(error @ ProviderVerificationError::Unavailable(_)) => {
            error.record_unavailable();
            Err(StoreError::Unavailable(
                "bearer authorization could not be verified".into(),
            ))
        }
    }
}

pub async fn resolve_subject(
    auth: &AuthState,
    metadata: &ProviderMetadata,
    access_token: &str,
) -> Result<(layer2_core::SubjectId, Vec<String>, Profile), String> {
    let verified = verify_subject(auth, metadata, access_token)
        .await
        .map_err(|error| error.to_string())?;
    match verified.permissions {
        permissions::PermissionEvidence::Active(permissions) => {
            Ok((verified.subject, permissions, verified.profile))
        }
        permissions::PermissionEvidence::Inactive => {
            Err("provider reports the access token is inactive".into())
        }
    }
}

pub async fn verify_subject(
    auth: &AuthState,
    metadata: &ProviderMetadata,
    access_token: &str,
) -> Result<VerifiedSubject, ProviderVerificationError> {
    verify_subject_expected(
        auth,
        metadata,
        access_token,
        None,
        tokio::time::Instant::now() + super::identity_http::DIRECT_TIMEOUT,
    )
    .await
}

pub(super) async fn verify_subject_expected(
    auth: &AuthState,
    metadata: &ProviderMetadata,
    access_token: &str,
    expected_subject: Option<&str>,
    deadline: tokio::time::Instant,
) -> Result<VerifiedSubject, ProviderVerificationError> {
    let result =
        verify_subject_once(auth, metadata, access_token, expected_subject, deadline).await;
    if matches!(
        &result,
        Err(ProviderVerificationError::Rejected(_) | ProviderVerificationError::Unavailable(_))
    ) {
        auth.identity_store.invalidate_token(access_token);
    }
    result
}

async fn verify_subject_once(
    auth: &AuthState,
    metadata: &ProviderMetadata,
    access_token: &str,
    expected_subject: Option<&str>,
    deadline: tokio::time::Instant,
) -> Result<VerifiedSubject, ProviderVerificationError> {
    if super::agent_keys::is_agent_key(access_token) {
        return super::agent_keys::verify(auth, metadata, access_token, deadline).await;
    }
    use ProviderVerificationError::{Rejected, Unavailable};
    if !super::bearer::permits_ambient_auth(access_token) {
        verification_failure("userinfo", "restricted_credential", None);
        return Err(Rejected(
            "credential is restricted to its dedicated channel interface".into(),
        ));
    }
    // Both requests inspect the same immutable credential. Bind the completed
    // introspection evidence to userinfo only after both have returned; retained
    // callers still recheck durable custody before accepting either result.
    let (userinfo, introspection) = tokio::try_join!(
        async {
            let started = std::time::Instant::now();
            let result = userinfo(auth, metadata, access_token, deadline)
                .await
                .and_then(|(body, subject)| {
                    if expected_subject.is_some_and(|expected| expected != subject) {
                        return claims(
                            Endpoint::Userinfo,
                            Err(Rejected(
                                "userinfo subject does not match retained owner".into(),
                            )),
                        );
                    }
                    Ok((body, subject))
                });
            tracing::info!(
                phase = "userinfo",
                elapsed_ms = started.elapsed().as_millis() as u64,
                success = result.is_ok(),
                "Identity provider verification phase completed"
            );
            // Transient errors must wait for the other endpoint: an explicit
            // rejection there takes precedence over retry.
            match result {
                Err(
                    error @ (ProviderVerificationError::Transient(_)
                    | ProviderVerificationError::RateLimited(_)),
                ) => Ok(Err(error)),
                result => result.map(Ok),
            }
        },
        async {
            let started = std::time::Instant::now();
            let result = permissions::fetch_introspection(auth, metadata, access_token, deadline)
                .await
                .and_then(|response| {
                    if let (Some(expected), Some(response)) = (expected_subject, response.as_ref())
                    {
                        claims(Endpoint::Introspection, response.require_subject(expected))?;
                    }
                    Ok(response)
                });
            tracing::info!(
                phase = "introspection",
                elapsed_ms = started.elapsed().as_millis() as u64,
                success = result.is_ok(),
                "Identity provider verification phase completed"
            );
            match result {
                Err(
                    error @ (ProviderVerificationError::Transient(_)
                    | ProviderVerificationError::RateLimited(_)),
                ) => Ok(Err(error)),
                result => result.map(Ok),
            }
        }
    )?;
    if let Some(error) = super::identity_throttle::select(&userinfo, &introspection) {
        return Err(error);
    }
    let (body, subject) = userinfo?;
    let (permissions, api_expires_at, token_expires_at) = match introspection?
        .map(|response| {
            response.verify(
                &subject,
                auth.oauth_api_resource.as_deref(),
                super::oauth_commands::requires_resource(access_token),
            )
        })
        .transpose()?
    {
        Some(evidence) => (
            evidence.permissions,
            evidence.api_expires_at,
            evidence.token_expires_at,
        ),
        None => (
            permissions::PermissionEvidence::Active(claims(
                Endpoint::Userinfo,
                permissions::permissions_claim(&body).map_err(Unavailable),
            )?),
            None,
            claims(Endpoint::Userinfo, permissions::token_expiry(&body))?,
        ),
    };
    Ok(VerifiedSubject {
        verification: None,
        subject: layer2_core::SubjectId::new(subject),
        permissions,
        profile: Profile::from_userinfo(&body),
        independent_commands: api_expires_at.is_some(),
        api_expires_at,
        token_expires_at: match (
            token_expires_at,
            claims(Endpoint::Userinfo, permissions::token_expiry(&body))?,
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        },
    })
}

async fn userinfo(
    auth: &AuthState,
    metadata: &ProviderMetadata,
    access_token: &str,
    deadline: tokio::time::Instant,
) -> Result<(serde_json::Value, String), ProviderVerificationError> {
    use ProviderVerificationError::Unavailable;
    let endpoint = configuration(
        Endpoint::Userinfo,
        metadata
            .userinfo_endpoint
            .as_ref()
            .ok_or_else(|| Unavailable("identity provider advertises no userinfo endpoint".into())),
    )?;
    let body = super::identity_cooldown::json(
        auth,
        access_token,
        endpoint,
        super::verification_diagnostics::Endpoint::Userinfo,
        auth.http.get(endpoint).bearer_auth(access_token),
        deadline,
    )
    .await?;
    claims(Endpoint::Userinfo, permissions::token_expiry(&body))?;
    let subject = claims(
        Endpoint::Userinfo,
        body.get("sub")
            .and_then(serde_json::Value::as_str)
            .filter(|subject| !subject.trim().is_empty())
            .ok_or_else(|| Unavailable("userinfo response has no subject".into())),
    )?
    .to_owned();
    Ok((body, subject))
}
