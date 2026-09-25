//! Aggregate-neutral NyxID key issuance.
//!
//! Resource services decide which durable aggregate owns a credential. This
//! client only resolves an exact service grant, asks NyxID for a bounded key,
//! validates the returned authority, and can later revoke or enumerate keys
//! owned by this deployment. Sandbox credential custody is introduced by #809.

use std::time::{Duration, SystemTime};

use futures_util::StreamExt as _;
use layer2_core::{ServiceId, StoreError};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::persistence::SecretValue;
pub(crate) use identifiers::valid_provider_key_id;
use identifiers::{credential_of, key_name, validate_credential_id};

mod bots;
pub(crate) use bots::owner_matches;
mod catalog;
mod identifiers;
mod provided;

const KEYS_PATH: &str = "/api/v1/api-keys";
const SERVICES_PATH: &str = "/api/v1/services";
const USER_SERVICES_PATH: &str = "/api/v1/user-services";
const KEY_SCOPES: &str = "proxy read";
const KEY_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const PROVIDER_KEY_LIST_MAX_BYTES: usize = 1024 * 1024;
const PROVIDER_KEY_LIST_MAX_ROWS: usize = 1_000;

#[derive(Debug, thiserror::Error)]
pub enum KeyIssuerError {
    #[error("credential identifier is not usable")]
    InvalidCredentialId,
    #[error("at least one service is required")]
    EmptyGrant,
    #[error("service is not granted: {0}")]
    ServiceNotGranted(ServiceId),
    #[error("identity service is unavailable: {0}")]
    Unavailable(String),
    #[error("identity service refused key operation with HTTP {0}")]
    Refused(u16),
    #[error("identity service returned a key with unexpected authority")]
    UnexpectedAuthority,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderKeyListingError {
    #[error("provider key listing request failed")]
    Request(#[from] reqwest::Error),
    #[error("provider key listing exceeded the {limit}-byte response limit")]
    TooManyBytes { limit: usize },
    #[error("provider key listing exceeded the {limit}-row limit")]
    TooManyRows { limit: usize },
    #[error("provider key listing response was not valid JSON")]
    InvalidJson(#[from] serde_json::Error),
}

/// A newly issued key. The plaintext value is redacted and zeroized on drop.
#[derive(Debug)]
pub struct MintedKey {
    pub id: String,
    pub value: SecretValue,
    pub expires_at: SystemTime,
    pub state_version: i64,
    pub grant_fingerprint: String,
}

/// A key name recognized as belonging to this deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedKey {
    pub id: String,
    pub credential_id: String,
}

/// One selectable platform service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableService {
    pub id: ServiceId,
    pub catalog_id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MintedKeyResponse {
    id: String,
    full_key: String,
    scopes: String,
    state_version: i64,
    allowed_service_ids: Vec<String>,
    allowed_node_ids: Vec<String>,
    allow_all_services: bool,
    allow_all_nodes: bool,
    #[serde(default)]
    allow_auto_connected_services: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ServiceRow {
    id: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct KeyRow {
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KeyListing {
    Bare(Vec<KeyRow>),
    Wrapped(WrappedKeyListing),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WrappedKeyListing {
    #[serde(alias = "api_keys", alias = "items")]
    keys: Vec<KeyRow>,
}

impl KeyListing {
    fn into_rows(self) -> Vec<KeyRow> {
        match self {
            Self::Bare(rows) => rows,
            Self::Wrapped(listing) => listing.keys,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ServiceListing {
    Bare(Vec<ServiceRow>),
    Wrapped {
        #[serde(alias = "items", alias = "user_services")]
        services: Vec<ServiceRow>,
    },
}

impl ServiceListing {
    fn into_rows(self) -> Vec<ServiceRow> {
        match self {
            Self::Bare(rows) => rows,
            Self::Wrapped { services } => services,
        }
    }
}

/// Thin HTTP client for the identity platform's service and key APIs.
pub struct KeyIssuer {
    http: reqwest::Client,
    base_url: String,
    deployment: String,
}

impl KeyIssuer {
    pub fn new(
        http: reqwest::Client,
        base_url: impl Into<String>,
        deployment: impl Into<String>,
    ) -> Self {
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            deployment: deployment.into(),
        }
    }

    /// Confirm that every named service is an active connection for the owner.
    pub async fn resolves_all(
        &self,
        owner_token: &str,
        wanted: &[ServiceId],
    ) -> Result<bool, StoreError> {
        self.resolve_exact(owner_token, wanted)
            .await
            .map(|_| true)
            .or_else(|error| match error {
                KeyIssuerError::ServiceNotGranted(_) | KeyIssuerError::EmptyGrant => Ok(false),
                other => Err(StoreError::Unavailable(other.to_string())),
            })
    }

    /// Mint one exact, expiring credential for an opaque resource identifier.
    pub async fn mint(
        &self,
        credential_id: &str,
        owner_token: &str,
        services: &[ServiceId],
    ) -> Result<MintedKey, KeyIssuerError> {
        validate_credential_id(credential_id)?;
        let allowed = self.resolve_exact(owner_token, services).await?;
        let expires_at = key_expiry(SystemTime::now());
        let body = mint_request_body(&self.deployment, credential_id, &allowed, expires_at)?;
        let response = self
            .http
            .post(format!("{}{KEYS_PATH}", self.base_url))
            .bearer_auth(owner_token)
            .json(&body)
            .send()
            .await
            .map_err(|error| KeyIssuerError::Unavailable(error.to_string()))?;
        if !response.status().is_success() {
            return Err(KeyIssuerError::Refused(response.status().as_u16()));
        }
        let response = response
            .json::<MintedKeyResponse>()
            .await
            .map_err(|error| KeyIssuerError::Unavailable(error.to_string()))?;
        minted_key_from_response(response, &allowed, expires_at)
            .ok_or(KeyIssuerError::UnexpectedAuthority)
    }

    /// Revoke a provider key using the authority of its owning account.
    pub async fn revoke(&self, id: &str, owner_token: &str) -> Result<(), KeyIssuerError> {
        if !valid_provider_key_id(id) {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let response = self
            .http
            .delete(format!("{}{KEYS_PATH}/{id}", self.base_url))
            .bearer_auth(owner_token)
            .send()
            .await
            .map_err(|_| {
                // Transport diagnostics can contain the provider key in the URL.
                KeyIssuerError::Unavailable("key revocation request failed".into())
            })?;
        revoke_status(response.status().as_u16())
    }

    /// List the platform catalogue as the owner who will use it.
    pub async fn services_of(
        &self,
        owner_token: &str,
    ) -> Result<Vec<AvailableService>, KeyIssuerError> {
        let rows = self.service_rows(SERVICES_PATH, owner_token).await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let id = row.slug.unwrap_or_else(|| row.id.clone());
                AvailableService {
                    catalog_id: row.id,
                    name: row.name.unwrap_or_else(|| id.clone()),
                    id: ServiceId::new(id),
                    description: row.description,
                }
            })
            .collect())
    }

    /// Enumerate keys whose names prove ownership by this deployment.
    pub async fn owned_keys(
        &self,
        owner_token: &str,
    ) -> Result<Vec<OwnedKey>, ProviderKeyListingError> {
        let response = self
            .http
            .get(format!("{}{KEYS_PATH}", self.base_url))
            .bearer_auth(owner_token)
            .send()
            .await?
            .error_for_status()?;
        if response
            .content_length()
            .is_some_and(|length| length > PROVIDER_KEY_LIST_MAX_BYTES as u64)
        {
            return Err(ProviderKeyListingError::TooManyBytes {
                limit: PROVIDER_KEY_LIST_MAX_BYTES,
            });
        }

        let mut body = Vec::with_capacity(
            response
                .content_length()
                .and_then(|length| usize::try_from(length).ok())
                .unwrap_or_default()
                .min(PROVIDER_KEY_LIST_MAX_BYTES),
        );
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk?;
            if body
                .len()
                .checked_add(chunk.len())
                .is_none_or(|length| length > PROVIDER_KEY_LIST_MAX_BYTES)
            {
                return Err(ProviderKeyListingError::TooManyBytes {
                    limit: PROVIDER_KEY_LIST_MAX_BYTES,
                });
            }
            body.extend_from_slice(&chunk);
        }

        let rows = serde_json::from_slice::<KeyListing>(&body)?.into_rows();
        if rows.len() > PROVIDER_KEY_LIST_MAX_ROWS {
            return Err(ProviderKeyListingError::TooManyRows {
                limit: PROVIDER_KEY_LIST_MAX_ROWS,
            });
        }
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                (valid_provider_key_id(&row.id))
                    .then(|| credential_of(&self.deployment, &row.name))
                    .flatten()
                    .map(|credential_id| OwnedKey {
                        id: row.id,
                        credential_id,
                    })
            })
            .collect())
    }

    async fn resolve_exact(
        &self,
        owner_token: &str,
        wanted: &[ServiceId],
    ) -> Result<Vec<String>, KeyIssuerError> {
        if wanted.is_empty() {
            return Err(KeyIssuerError::EmptyGrant);
        }
        let rows = self.service_rows(USER_SERVICES_PATH, owner_token).await?;
        let mut resolved = Vec::with_capacity(wanted.len());
        for wanted_service in wanted {
            let row = rows
                .iter()
                .find(|row| {
                    row.id == wanted_service.0
                        || row.slug.as_deref() == Some(wanted_service.0.as_str())
                })
                .filter(|row| row.is_active == Some(true))
                .ok_or_else(|| KeyIssuerError::ServiceNotGranted(wanted_service.clone()))?;
            if !resolved.contains(&row.id) {
                resolved.push(row.id.clone());
            }
        }
        if resolved.is_empty() {
            return Err(KeyIssuerError::EmptyGrant);
        }
        Ok(resolved)
    }

    async fn service_rows(
        &self,
        path: &str,
        owner_token: &str,
    ) -> Result<Vec<ServiceRow>, KeyIssuerError> {
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
            .bearer_auth(owner_token)
            .send()
            .await
            .map_err(|error| KeyIssuerError::Unavailable(error.to_string()))?;
        if !response.status().is_success() {
            return Err(KeyIssuerError::Refused(response.status().as_u16()));
        }
        catalog::rows(response).await
    }
}

fn revoke_status(status: u16) -> Result<(), KeyIssuerError> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(KeyIssuerError::Refused(status))
    }
}

fn key_expiry(now: SystemTime) -> SystemTime {
    mongodb::bson::DateTime::from_system_time(now + KEY_LIFETIME).to_system_time()
}

fn mint_request_body(
    deployment: &str,
    credential_id: &str,
    allowed_service_ids: &[String],
    expires_at: SystemTime,
) -> Result<serde_json::Value, KeyIssuerError> {
    let expires_at = mongodb::bson::DateTime::from_system_time(expires_at)
        .try_to_rfc3339_string()
        .map_err(|error| KeyIssuerError::Unavailable(error.to_string()))?;
    Ok(serde_json::json!({
        "name": key_name(deployment, credential_id)?,
        "scopes": KEY_SCOPES,
        "expires_at": expires_at,
        "allowed_service_ids": allowed_service_ids,
        "allow_all_services": false,
        "allow_all_nodes": false,
    }))
}

fn normalized_identifiers(values: &[String]) -> Option<Vec<&str>> {
    let mut normalized = Vec::with_capacity(values.len());
    for value in values {
        let value = value.trim();
        if value.is_empty()
            || value
                .bytes()
                .filter(u8::is_ascii_hexdigit)
                .all(|byte| byte == b'0')
            || normalized.contains(&value)
        {
            return None;
        }
        normalized.push(value);
    }
    normalized.sort_unstable();
    Some(normalized)
}

fn provider_grant_fingerprint(response: &MintedKeyResponse) -> Option<String> {
    let services = normalized_identifiers(&response.allowed_service_ids)?;
    let nodes = normalized_identifiers(&response.allowed_node_ids)?;
    let mut digest = Sha256::new();
    digest.update(b"cma-nyxid-resource-key-grant-v1\0");
    let scopes = response.scopes.trim();
    digest.update((scopes.len() as u64).to_be_bytes());
    digest.update(scopes.as_bytes());
    for collection in [services, nodes] {
        digest.update((collection.len() as u64).to_be_bytes());
        for value in collection {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
    }
    digest.update([u8::from(response.allow_all_services)]);
    digest.update([u8::from(response.allow_all_nodes)]);
    Some(format!("{:x}", digest.finalize()))
}

fn minted_key_from_response(
    response: MintedKeyResponse,
    expected_service_ids: &[String],
    expires_at: SystemTime,
) -> Option<MintedKey> {
    let expected = normalized_identifiers(expected_service_ids)?;
    let returned = normalized_identifiers(&response.allowed_service_ids)?;
    if !valid_provider_key_id(&response.id)
        || response.full_key.trim().is_empty()
        || response.scopes != KEY_SCOPES
        || response.state_version <= 0
        || returned != expected
        || !response.allowed_node_ids.is_empty()
        || response.allow_all_services
        || response.allow_all_nodes
        || response.allow_auto_connected_services
    {
        return None;
    }
    let grant_fingerprint = provider_grant_fingerprint(&response)?;
    Some(MintedKey {
        id: response.id,
        value: SecretValue::new(response.full_key),
        expires_at,
        state_version: response.state_version,
        grant_fingerprint,
    })
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
