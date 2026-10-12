//! NyxID introspection for this service's own resource and permissions. The
//! response is obtained from the configured issuer over an authenticated
//! channel; unverified JWT payloads and caller-supplied owner fields are unused.

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::http::{header::AUTHORIZATION, HeaderMap};
use serde::Deserialize;

use crate::{model::bounded_text, permissions};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("valid Sandbox Manager authorization is required")]
    Unauthenticated,
    #[error("Sandbox Manager permission is required")]
    Forbidden,
    #[error("NyxID authorization is temporarily unavailable")]
    Unavailable,
    #[error("invalid NyxID configuration")]
    Configuration,
}

#[derive(Debug, Clone)]
pub struct Principal {
    pub subject: String,
    pub permissions: Vec<String>,
    pub expires_at: u64,
}

impl Principal {
    pub fn require(&self, permission: &str) -> Result<(), AuthError> {
        if permissions::allows(&self.permissions, permission) {
            Ok(())
        } else {
            Err(AuthError::Forbidden)
        }
    }
}

#[derive(Clone)]
pub struct NyxIdAuth {
    http: reqwest::Client,
    issuer: String,
    endpoint: url::Url,
    resource: String,
    client_id: String,
    client_secret: Arc<str>,
}

#[derive(Deserialize)]
struct Introspection {
    active: bool,
    sub: Option<String>,
    iss: Option<String>,
    exp: Option<u64>,
    token_type: Option<String>,
    resource: Option<Vec<String>>,
    permissions: Option<Vec<String>>,
}

impl NyxIdAuth {
    pub fn new(
        issuer: &str,
        resource: &str,
        client_id: &str,
        client_secret: String,
    ) -> Result<Self, AuthError> {
        let url = url::Url::parse(issuer).map_err(|_| AuthError::Configuration)?;
        let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || !bounded_text(client_id, 256)
            || client_secret.is_empty()
            || client_secret.len() > 8192
        {
            return Err(AuthError::Configuration);
        }
        let resource_url = url::Url::parse(resource).map_err(|_| AuthError::Configuration)?;
        // NyxID resource identifiers are its own service proxy URIs. A different
        // issuer or service cannot become an alias of this service by accident.
        if resource_url.origin() != url.origin()
            || resource_url.query().is_some()
            || resource_url.fragment().is_some()
            || !resource_url.username().is_empty()
            || resource_url.password().is_some()
            || resource_url.path() != "/api/v1/proxy/s/cma-sandbox-manager"
        {
            return Err(AuthError::Configuration);
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(4))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| AuthError::Configuration)?;
        Ok(Self {
            http,
            issuer: issuer.trim_end_matches('/').into(),
            endpoint: url
                .join("/oauth/introspect")
                .map_err(|_| AuthError::Configuration)?,
            resource: resource.to_owned(),
            client_id: client_id.to_owned(),
            client_secret: client_secret.into(),
        })
    }

    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError> {
        if headers.get_all(AUTHORIZATION).iter().count() != 1 {
            return Err(AuthError::Unauthenticated);
        }
        let token = headers
            .get(AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .filter(|t| !t.is_empty() && t.len() <= 16_384 && !t.chars().any(char::is_whitespace))
            .ok_or(AuthError::Unauthenticated)?;
        let mut response = self
            .http
            .post(self.endpoint.clone())
            .form(&[
                ("token", token),
                ("token_type_hint", "access_token"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_ref()),
            ])
            .send()
            .await
            .map_err(|_| AuthError::Unavailable)?;
        // This HTTP status authenticates our confidential client. An invalid
        // user token is the successful inactive response, not a provider error.
        if !response.status().is_success() {
            return Err(AuthError::Unavailable);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| AuthError::Unavailable)? {
            if body.len() + chunk.len() > 65_536 {
                return Err(AuthError::Unavailable);
            }
            body.extend_from_slice(&chunk);
        }
        let observation: Introspection =
            serde_json::from_slice(&body).map_err(|_| AuthError::Unavailable)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AuthError::Unavailable)?
            .as_secs();
        self.validate(observation, now)
    }

    fn validate(&self, observation: Introspection, now: u64) -> Result<Principal, AuthError> {
        if !observation.active {
            return Err(AuthError::Unauthenticated);
        }
        if observation.iss.as_deref() != Some(&self.issuer)
            || observation.token_type.as_deref() != Some("access")
            || observation.exp.is_none_or(|exp| exp <= now)
            || !observation
                .resource
                .as_ref()
                .is_some_and(|resources| resources.contains(&self.resource))
        {
            return Err(AuthError::Unauthenticated);
        }
        let subject = observation
            .sub
            .filter(|s| bounded_text(s, 512))
            .ok_or(AuthError::Unauthenticated)?;
        let permissions = observation.permissions.ok_or(AuthError::Unavailable)?;
        Ok(Principal {
            subject,
            permissions,
            expires_at: observation.exp.ok_or(AuthError::Unauthenticated)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> NyxIdAuth {
        NyxIdAuth::new(
            "https://identity.example",
            "https://identity.example/api/v1/proxy/s/cma-sandbox-manager",
            "sandbox-client",
            "unused".into(),
        )
        .unwrap()
    }

    fn evidence() -> Introspection {
        Introspection {
            active: true,
            sub: Some("owner".into()),
            iss: Some("https://identity.example".into()),
            exp: Some(200),
            token_type: Some("access".into()),
            resource: Some(vec![
                "https://identity.example/api/v1/proxy/s/cma-sandbox-manager".into(),
            ]),
            permissions: Some(vec![permissions::CREATE.into()]),
        }
    }

    #[test]
    fn target_expiry_type_and_complete_permissions_are_required() {
        let auth = auth();
        auth.validate(evidence(), 100)
            .unwrap()
            .require(permissions::CREATE)
            .unwrap();
        for field in [
            "active",
            "subject",
            "issuer",
            "expiry",
            "type",
            "resource",
            "permissions",
        ] {
            let mut claim = evidence();
            match field {
                "active" => claim.active = false,
                "subject" => claim.sub = Some(" ".into()),
                "issuer" => claim.iss = Some("https://other.example".into()),
                "expiry" => claim.exp = Some(100),
                "type" => claim.token_type = Some("refresh".into()),
                "resource" => {
                    claim.resource = Some(vec![
                        "https://identity.example/api/v1/proxy/s/cma-agent-manager".into(),
                    ])
                }
                "permissions" => claim.permissions = None,
                _ => unreachable!(),
            }
            assert!(
                auth.validate(claim, 100).is_err(),
                "accepted invalid {field}"
            );
        }
        let mut claim = evidence();
        claim.resource = None;
        assert!(auth.validate(claim, 100).is_err());
        let mut claim = evidence();
        claim.permissions = Some(vec!["cma-am:admin".into()]);
        assert_eq!(
            auth.validate(claim, 100)
                .unwrap()
                .require(permissions::CREATE),
            Err(AuthError::Forbidden)
        );
    }
}
