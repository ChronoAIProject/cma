//! Provider verification for credentials CMA did not issue.

use layer2_core::StoreError;

use super::verification_error::ProviderVerificationError;
use super::{permissions, profile::Profile, AuthState, ProviderMetadata, RequestAuth};

pub(super) struct VerifiedSubject {
    pub subject: layer2_core::SubjectId,
    pub permissions: permissions::PermissionEvidence,
    pub profile: Profile,
    pub independent_commands: bool,
    pub api_expires_at: Option<std::time::SystemTime>,
}

pub(super) fn verification_failure(stage: &'static str, reason: &'static str, status: Option<u16>) {
    tracing::warn!(
        stage,
        reason,
        status,
        "identity provider verification failed"
    );
}

pub(super) async fn request_for_bearer(
    auth: &AuthState,
    token: String,
) -> Result<Option<RequestAuth>, StoreError> {
    let metadata = match auth.metadata().await {
        Ok(metadata) => metadata,
        Err(error) => {
            tracing::warn!(error = %error, "could not discover identity provider for bearer verification");
            return Err(StoreError::Unavailable(
                "identity provider discovery is unavailable".into(),
            ));
        }
    };
    match super::bearer_authorization::verify_subject(auth, &metadata, &token).await {
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
        Err(ProviderVerificationError::Unavailable(_)) => Err(StoreError::Unavailable(
            "bearer authorization could not be verified".into(),
        )),
    }
}

pub(super) async fn resolve_subject(
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

pub(super) async fn verify_subject(
    auth: &AuthState,
    metadata: &ProviderMetadata,
    access_token: &str,
) -> Result<VerifiedSubject, ProviderVerificationError> {
    if super::agent_keys::is_agent_key(access_token) {
        return super::agent_keys::verify(auth, metadata, access_token).await;
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
    let ((body, subject), introspection) = tokio::try_join!(
        async {
            let started = std::time::Instant::now();
            let result = userinfo(auth, metadata, access_token).await;
            tracing::info!(
                phase = "userinfo",
                elapsed_ms = started.elapsed().as_millis() as u64,
                success = result.is_ok(),
                "Identity provider verification phase completed"
            );
            result
        },
        async {
            let started = std::time::Instant::now();
            let result = permissions::fetch_introspection(auth, metadata, access_token).await;
            tracing::info!(
                phase = "introspection",
                elapsed_ms = started.elapsed().as_millis() as u64,
                success = result.is_ok(),
                "Identity provider verification phase completed"
            );
            // Only userinfo can fail the join: a prompt rejection must cancel
            // pending introspection, and introspection failure cannot preempt
            // userinfo's existing refusal precedence.
            Ok::<_, ProviderVerificationError>(result)
        }
    )?;
    let (permissions, api_expires_at) = match introspection?
        .map(|response| response.verify(&subject))
        .transpose()?
    {
        Some(evidence) => (evidence.permissions, evidence.api_expires_at),
        None => (
            permissions::PermissionEvidence::Active(
                permissions::permissions_claim(&body).map_err(Unavailable)?,
            ),
            None,
        ),
    };
    Ok(VerifiedSubject {
        subject: layer2_core::SubjectId::new(subject),
        permissions,
        profile: Profile::from_userinfo(&body),
        independent_commands: api_expires_at.is_some(),
        api_expires_at,
    })
}

async fn userinfo(
    auth: &AuthState,
    metadata: &ProviderMetadata,
    access_token: &str,
) -> Result<(serde_json::Value, String), ProviderVerificationError> {
    use ProviderVerificationError::{Rejected, Unavailable};
    let endpoint = metadata
        .userinfo_endpoint
        .as_ref()
        .ok_or_else(|| Unavailable("identity provider advertises no userinfo endpoint".into()))?;
    let response = auth
        .http
        .get(endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|error| {
            verification_failure("userinfo", "transport", None);
            Unavailable(format!("userinfo request failed: {error}"))
        })?;
    if !response.status().is_success() {
        verification_failure("userinfo", "http_status", Some(response.status().as_u16()));
        let message = format!("userinfo returned HTTP {}", response.status().as_u16());
        return Err(if matches!(response.status().as_u16(), 401 | 403) {
            Rejected(message)
        } else {
            Unavailable(message)
        });
    }
    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| {
            verification_failure("userinfo", "invalid_json", None);
            Unavailable(format!("userinfo response is invalid: {error}"))
        })?;
    let subject = body
        .get("sub")
        .and_then(serde_json::Value::as_str)
        .filter(|subject| !subject.trim().is_empty())
        .ok_or_else(|| Unavailable("userinfo response has no subject".into()))?
        .to_owned();
    Ok((body, subject))
}
