//! Provider verification for credentials CMA did not issue.

use layer2_core::StoreError;

use super::{permissions, profile::Profile, AuthState, ProviderMetadata, RequestAuth};

pub(super) struct VerifiedSubject {
    pub subject: layer2_core::SubjectId,
    pub permissions: permissions::PermissionEvidence,
    pub profile: Profile,
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
            return Ok(None);
        }
    };
    match resolve_subject(auth, &metadata, &token).await {
        Ok((subject, permission_names, profile)) => Ok(Some(RequestAuth {
            subject,
            authorization: crate::permissions::capabilities_from_permissions(permission_names),
            profile,
            delegated_credential: Some(token),
        })),
        Err(error) => {
            tracing::debug!(error = %error, "bearer credential was not accepted");
            Ok(None)
        }
    }
}

pub(super) async fn resolve_subject(
    auth: &AuthState,
    metadata: &ProviderMetadata,
    access_token: &str,
) -> Result<(layer2_core::SubjectId, Vec<String>, Profile), String> {
    let verified = verify_subject(auth, metadata, access_token).await?;
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
) -> Result<VerifiedSubject, String> {
    if !super::bearer::permits_ambient_auth(access_token) {
        verification_failure("userinfo", "restricted_credential", None);
        return Err("credential is restricted to its dedicated channel interface".into());
    }
    let endpoint = metadata
        .userinfo_endpoint
        .as_ref()
        .ok_or_else(|| "identity provider advertises no userinfo endpoint".to_string())?;
    let response = auth
        .http
        .get(endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|error| {
            verification_failure("userinfo", "transport", None);
            format!("userinfo request failed: {error}")
        })?;
    if !response.status().is_success() {
        verification_failure("userinfo", "http_status", Some(response.status().as_u16()));
        return Err(format!(
            "userinfo returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| {
            verification_failure("userinfo", "invalid_json", None);
            format!("userinfo response is invalid: {error}")
        })?;
    let subject = body
        .get("sub")
        .and_then(serde_json::Value::as_str)
        .filter(|subject| !subject.trim().is_empty())
        .ok_or_else(|| "userinfo response has no subject".to_string())?;
    let permissions =
        match permissions::introspect_permissions(auth, metadata, access_token, subject).await? {
            Some(evidence) => evidence,
            None => permissions::PermissionEvidence::Active(permissions::permissions_claim(&body)?),
        };
    Ok(VerifiedSubject {
        subject: layer2_core::SubjectId::new(subject),
        permissions,
        profile: Profile::from_userinfo(&body),
    })
}
