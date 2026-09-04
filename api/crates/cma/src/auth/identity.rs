//! Provider verification for credentials CMA did not issue.

use layer2_core::StoreError;

use super::{permissions, profile::Profile, AuthState, ProviderMetadata, RequestAuth};

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
        .map_err(|error| format!("userinfo request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "userinfo returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("userinfo response is invalid: {error}"))?;
    let subject = body
        .get("sub")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|subject| !subject.is_empty())
        .ok_or_else(|| "userinfo response has no subject".to_string())?;
    let permissions = permissions::introspect_permissions(auth, metadata, access_token)
        .await?
        .unwrap_or_else(|| permissions::permissions_claim(&body));
    Ok((
        layer2_core::SubjectId::new(subject),
        permissions,
        Profile::from_userinfo(&body),
    ))
}
