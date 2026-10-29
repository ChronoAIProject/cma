//! Ordinary personal keys have a different introspection contract from OAuth.
//! This verifies identity only; a service must separately exclude worker keys.
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, header::AUTHORIZATION};
use serde_json::Value;

use crate::{AuthError, NyxIdAuth, bounded_text};

pub fn is_service_key(token: &str) -> bool {
    token.starts_with("nyx_") || token.starts_with("nyxid_ag_")
}

#[derive(Clone, Debug)]
pub struct ServiceKeyAuth {
    identity: NyxIdAuth,
    service_id: String,
}

/// No fabricated expiry: NyxID may explicitly verify a nonexpiring key.
#[derive(Clone, Debug)]
pub struct ServiceKeyPrincipal {
    pub subject: String,
    pub permissions: Vec<String>,
    pub issued_at: u64,
    pub key_id: String,
    pub expires_at: Option<u64>,
    admin_permission: String,
}

impl ServiceKeyPrincipal {
    pub fn require(&self, permission: &str) -> Result<(), AuthError> {
        if self
            .permissions
            .iter()
            .any(|value| value == permission || value == &self.admin_permission)
        {
            Ok(())
        } else {
            Err(AuthError::Forbidden)
        }
    }
}

impl ServiceKeyAuth {
    pub fn new(identity: NyxIdAuth, service_id: &str) -> Result<Self, AuthError> {
        // Service UUID, not a catalog slug or an RFC 8707 resource URI.
        if service_id.len() != 36
            || !service_id.bytes().enumerate().all(|(index, value)| {
                if [8, 13, 18, 23].contains(&index) {
                    value == b'-'
                } else {
                    value.is_ascii_digit() || (b'a'..=b'f').contains(&value)
                }
            })
            || service_id == "00000000-0000-0000-0000-000000000000"
        {
            return Err(AuthError::Configuration);
        }
        Ok(Self {
            identity,
            service_id: service_id.into(),
        })
    }

    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<ServiceKeyPrincipal, AuthError> {
        if headers.get_all(AUTHORIZATION).iter().count() != 1 {
            return Err(AuthError::Unauthenticated);
        }
        let token = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| {
                is_service_key(value)
                    && value.len() <= 16_384
                    && !value.chars().any(char::is_whitespace)
            })
            .ok_or(AuthError::Unauthenticated)?;
        let mut response = self
            .identity
            .http
            .post(format!("{}/oauth/introspect", self.identity.issuer))
            .form(&[
                ("token", token),
                ("token_type_hint", "api_key"),
                ("service_id", self.service_id.as_str()),
                ("client_id", self.identity.client_id.as_str()),
                ("client_secret", self.identity.client_secret.as_ref()),
            ])
            .send()
            .await
            .map_err(|_| AuthError::Unavailable)?;
        // HTTP rejection is a resource-server/client failure. Revoked user
        // credentials arrive as successful introspection with active=false.
        if !response.status().is_success() {
            return Err(AuthError::Unavailable);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| AuthError::Unavailable)? {
            if bytes.len() + chunk.len() > 65_536 {
                return Err(AuthError::Unavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        let evidence = serde_json::from_slice(&bytes).map_err(|_| AuthError::Unavailable)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AuthError::Unavailable)?
            .as_secs();
        self.validate(&evidence, now)
    }

    fn validate(&self, evidence: &Value, now: u64) -> Result<ServiceKeyPrincipal, AuthError> {
        match evidence.get("active").and_then(Value::as_bool) {
            Some(false) => return Err(AuthError::Unauthenticated),
            Some(true) => (),
            None => return Err(AuthError::Unavailable),
        }
        if evidence.get("iss").and_then(Value::as_str) != Some(&self.identity.issuer)
            || evidence.get("aud").and_then(Value::as_str) != Some(&self.service_id)
            || evidence.get("token_type").and_then(Value::as_str) != Some("api_key")
            || !evidence
                .get("scope")
                .and_then(Value::as_str)
                .is_some_and(|scope| scope.split_whitespace().any(|value| value == "proxy"))
            || evidence
                .get("resource")
                .is_some_and(|value| !value.is_null())
        {
            return Err(AuthError::Unauthenticated);
        }
        let text = |field| {
            evidence
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| bounded_text(value, 512))
                .map(str::to_owned)
                .ok_or(AuthError::Unauthenticated)
        };
        let subject = text("sub")?;
        let key_id = text("jti")?;
        let issued_at = evidence
            .get("iat")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0 && *value <= now)
            .ok_or(AuthError::Unauthenticated)?;
        let expires_at = match evidence.get("exp") {
            None => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .filter(|value| *value > now && *value > issued_at)
                    .ok_or(AuthError::Unauthenticated)?,
            ),
        };
        let permissions = evidence
            .get("permissions")
            .and_then(Value::as_array)
            .filter(|values| values.len() <= 512)
            .ok_or(AuthError::Unavailable)?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| bounded_text(value, 256))
                    .map(str::to_owned)
                    .ok_or(AuthError::Unavailable)
            })
            .collect::<Result<_, _>>()?;
        Ok(ServiceKeyPrincipal {
            subject,
            permissions,
            issued_at,
            key_id,
            expires_at,
            admin_permission: format!("{}:admin", self.identity.service.permission_prefix()),
        })
    }
}

#[cfg(test)]
#[path = "service_key_tests.rs"]
mod tests;
