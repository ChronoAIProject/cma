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
pub use identifiers::valid_provider_key_id;
use identifiers::{credential_of, key_name, validate_credential_id};

mod bots;
pub use bots::owner_matches;
mod catalog;
mod dispatch;
pub use dispatch::{MintDispatchFailure, ResolvedMintService};
mod grant;
mod identifiers;
#[cfg(test)]
mod nyxid_contract_tests;
mod provided;
mod retirement;
pub use retirement::CredentialRetirementReceipt;
mod scope_plan;
use grant::minted_key_from_response;
#[cfg(any(test, feature = "test-support"))]
pub use scope_plan::tests::scope_plan_fixture;

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
    allowed_service_ids: Vec<String>,
    allowed_node_ids: Vec<String>,
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
    catalog_service_id: Option<String>,
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

    /// Prove current service and backing-node authority before owner admission.
    pub async fn confirm_service_scope(
        &self,
        owner: &layer2_core::SubjectId,
        token: &str,
        services: &[ServiceId],
    ) -> Result<(), KeyIssuerError> {
        let allowed = self.resolve_exact(token, services).await?;
        self.scope_plan(
            token,
            &allowed,
            &layer2_core::resource_access::ResourceOwner::Person {
                subject: owner.clone(),
            },
        )
        .await
        .map(|_| ())
    }

    /// Mint one exact, expiring credential for an opaque resource identifier.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn mint(
        &self,
        credential_id: &str,
        owner: &layer2_core::SubjectId,
        owner_token: &str,
        services: &[ServiceId],
    ) -> Result<MintedKey, KeyIssuerError> {
        self.prepare_mint(credential_id, owner, owner_token, services)
            .await?
            .dispatch()
            .await
            .map_err(|failure| failure.error)
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
        Ok(Self::resolve_service_rows(&rows, wanted)?
            .into_iter()
            .map(|row| row.id.clone())
            .collect())
    }

    fn resolve_service_rows<'a>(
        rows: &'a [ServiceRow],
        wanted: &[ServiceId],
    ) -> Result<Vec<&'a ServiceRow>, KeyIssuerError> {
        let mut resolved = Vec::with_capacity(wanted.len());
        for wanted_service in wanted {
            let row = rows
                .iter()
                .find(|row| {
                    row.id == wanted_service.0
                        || row.slug.as_deref() == Some(wanted_service.as_str())
                })
                .filter(|row| row.is_active == Some(true))
                .ok_or_else(|| KeyIssuerError::ServiceNotGranted(wanted_service.clone()))?;
            if !resolved
                .iter()
                .any(|selected: &&ServiceRow| selected.id == row.id)
            {
                resolved.push(row);
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
    bson::DateTime::from_system_time(now + KEY_LIFETIME).to_system_time()
}

fn mint_request_body(
    deployment: &str,
    credential_id: &str,
    allowed_service_ids: &[String],
    expires_at: SystemTime,
) -> Result<serde_json::Value, KeyIssuerError> {
    let expires_at = bson::DateTime::from_system_time(expires_at)
        .try_to_rfc3339_string()
        .map_err(|error| KeyIssuerError::Unavailable(error.to_string()))?;
    Ok(serde_json::json!({
        "name": key_name(deployment, credential_id)?,
        "scopes": KEY_SCOPES,
        "expires_at": expires_at,
        "allowed_service_ids": allowed_service_ids,
        "allow_all_services": false,
        "allow_all_nodes": false,
        "allow_auto_connected_services": false,
    }))
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
