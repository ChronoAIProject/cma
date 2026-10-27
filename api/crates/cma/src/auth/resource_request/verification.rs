use std::time::{Duration, SystemTime};

use layer2_core::{Authorization, SubjectId};
use serde_json::Value;

use super::{descriptor::Descriptor, AuthMode, AuthState};

pub(super) enum Error {
    Rejected,
    Unavailable,
}

pub(super) struct Verified {
    pub subject: SubjectId,
    pub authorization: Authorization,
    pub expires_at: SystemTime,
}

pub(super) async fn verify(
    auth: &AuthState,
    proof: &str,
    descriptor: &Descriptor,
) -> Result<Verified, Error> {
    let AuthMode::Provider {
        client_id,
        client_secret,
        ..
    } = &auth.mode
    else {
        return Err(Error::Rejected);
    };
    let service_id = auth
        .agent_key_service_id
        .as_deref()
        .ok_or(Error::Rejected)?;
    let metadata = auth.metadata().await.map_err(|_| Error::Unavailable)?;
    let endpoint = metadata
        .introspection_endpoint
        .as_deref()
        .ok_or(Error::Unavailable)?;
    let mut form = vec![
        ("token", proof),
        ("token_type_hint", "resource_request"),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("service_id", service_id),
        ("request_method", &descriptor.method),
        ("request_path_and_query", &descriptor.path_and_query),
        ("request_body_sha256", descriptor.body_sha256),
    ];
    if let Some(content_type) = descriptor.content_type.as_deref() {
        form.push(("request_content_type", content_type));
    }
    let mut response = auth
        .http
        .post(endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|_| Error::Unavailable)?;
    if !response.status().is_success() {
        return Err(Error::Unavailable);
    }
    const MAX_EVIDENCE: usize = 64 * 1024;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_EVIDENCE as u64)
    {
        return Err(Error::Unavailable);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_EVIDENCE {
            return Err(Error::Unavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)?;
    validate(body, service_id, descriptor, SystemTime::now())
}

fn validate(
    body: Value,
    service_id: &str,
    descriptor: &Descriptor,
    now: SystemTime,
) -> Result<Verified, Error> {
    match body.get("active").and_then(Value::as_bool) {
        Some(true) => {}
        Some(false) => return Err(Error::Rejected),
        None => return Err(Error::Unavailable),
    }
    if body.get("aud").and_then(Value::as_str) != Some(service_id)
        || body.get("token_type").and_then(Value::as_str) != Some("resource_request")
        || body.get("independent_commands").and_then(Value::as_bool) != Some(false)
        || body.get("resource_request")
            != Some(&serde_json::to_value(descriptor).map_err(|_| Error::Unavailable)?)
        || !body
            .get("scope")
            .and_then(Value::as_str)
            .is_some_and(|scopes| scopes.split_whitespace().any(|scope| scope == "proxy"))
    {
        return Err(Error::Rejected);
    }
    // The configured authenticated introspection endpoint establishes issuer
    // authority; transport/public origins need not equal the provider's issuer.
    if body
        .get("iss")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
        || body
            .get("jti")
            .and_then(Value::as_str)
            .is_none_or(|value| uuid::Uuid::parse_str(value).is_err())
    {
        return Err(Error::Rejected);
    }
    let subject = body
        .get("sub")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.trim() == *value && value.len() <= 256)
        .ok_or(Error::Rejected)?;
    let now_seconds = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| Error::Unavailable)?
        .as_secs();
    let exp = body
        .get("exp")
        .and_then(Value::as_u64)
        .ok_or(Error::Rejected)?;
    let iat = body
        .get("iat")
        .and_then(Value::as_u64)
        .ok_or(Error::Rejected)?;
    if exp <= now_seconds
        || iat > now_seconds.saturating_add(5)
        || !exp
            .checked_sub(iat)
            .is_some_and(|lifetime| (1..=60).contains(&lifetime))
    {
        return Err(Error::Rejected);
    }
    let expires_at = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(exp))
        .ok_or(Error::Rejected)?;
    let permissions =
        crate::auth::permissions::permissions_claim(&body).map_err(|_| Error::Unavailable)?;
    Ok(Verified {
        subject: SubjectId::new(subject),
        authorization: crate::permissions::capabilities_from_permissions(permissions),
        expires_at,
    })
}
