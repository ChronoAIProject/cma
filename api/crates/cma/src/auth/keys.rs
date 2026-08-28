//! Credentials minted for one session, reaching only what that session needs.
//!
//! A session acts on its owner's behalf. Until now it did so holding the
//! owner's own sign-in token, which reaches everything they can reach — every
//! service on the platform, whether or not the work called for it. A profile
//! that says "this session may use these services" cannot be honoured by a
//! credential that was never narrowed.
//!
//! So the control plane mints one instead: a key belonging to the owner,
//! limited to the services their profile named, and belonging to that session
//! alone. What a session may do stops being a property of who started it and
//! becomes a property of the work it was started for.
//!
//! **The allowance is a floor, not a default.** One credential serves the
//! agent's model calls and the platform CLI alike, so a selection that omitted
//! the model endpoint would not produce a narrower session but one that cannot
//! think. What the deployment mandates is added whatever the profile says.
//!
//! **The key's name is its ownership record.** Nothing else ties a key to the
//! session it was minted for, so the name carries the deployment and the
//! session, and cleanup reads it back. That survives a restart, which a note
//! held in memory would not.
//!
//! **Every new key requests a provider expiry; enforcement evidence is tracked
//! by #524.** NyxID accepts and stores one exact absolute expiry thirty days
//! after minting, but the staging/canary gate must still prove that proxy use is
//! denied after that instant. A running session starts replacement after twelve
//! hours, leaving twenty-nine and a half days between replacement and the
//! requested bound. Sessions may run indefinitely by rotating credentials.
//!
//! A requested expiry is not permission to abandon revocation. Until #524
//! demonstrates provider enforcement, a key we fail to take back is treated as
//! a live `proxy read` credential, and legacy keys created without an expiry
//! remain live until explicitly deleted. Every failed revoke therefore stays
//! visible to the durable reconciliation paths.

use futures_util::StreamExt as _;
use layer2_core::{ServiceId, SessionId, SubjectId};
use serde::Deserialize;
use std::time::{Duration, SystemTime};

/// Where keys are minted, relative to the platform root.
const KEYS_PATH: &str = "/api/v1/api-keys";

/// Ownership prefix on every key minted after the CMA migration.
const KEY_NAME_PREFIX: &str = "cma";

/// Ownership prefix on keys minted before the CMA migration.
///
/// These credentials do not expire, so reclamation must keep recognising the
/// old prefix until provider listings prove none remain on any owner's account.
const LEGACY_KEY_NAME_PREFIX: &str = "chrono-bot";

/// Where the platform's whole catalogue of services is listed.
///
/// What a person choosing among services is shown: it carries the names and
/// descriptions that make a service recognisable, which the per-owner listing
/// below does not.
const SERVICES_PATH: &str = "/api/v1/services";

/// Where *this owner's connected* services are listed.
///
/// **Deliberately a different route from the catalogue above, and the
/// difference is why minting never once succeeded.** The two are separate id
/// spaces for the same service — `chrono-llm` is one identifier in the
/// catalogue and another here — and `allowed_service_ids` on the minting call
/// means this one. Sending a catalogue identifier is refused with
/// `UserService '…' not found or not permitted for this API key`, every time,
/// for every profile, which is what production had been doing since the
/// feature shipped.
///
/// It is the correct source on its own terms too: a profile naming something
/// its owner has never connected resolves to nothing here, where against the
/// catalogue it resolves happily and mints a key claiming reach the owner does
/// not have.
const USER_SERVICES_PATH: &str = "/api/v1/user-services";

/// What the minted key may do, beyond reaching its services.
///
/// Proxying and reading, and nothing that manages the account. A session is
/// meant to act through the platform, not to reconfigure it — and a credential
/// that could edit its own allowance would not be an allowance.
const KEY_SCOPES: &str = "proxy read";

/// Requested provider lifetime of one session credential.
///
/// Session continuity does not depend on one key surviving this long. The
/// renewal loop starts durable replacement after twelve hours, so this bound
/// leaves 29.5 days for retries before the requested expiry while #524 verifies
/// that the provider enforces that bound.
pub(crate) const SESSION_KEY_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);

fn session_key_expiry(now: SystemTime) -> SystemTime {
    // NyxID persists API-key expiry at millisecond precision. Round before
    // sending and retaining it so CMA never believes the key lives even a
    // fraction longer than the provider does.
    mongodb::bson::DateTime::from_system_time(now + SESSION_KEY_LIFETIME).to_system_time()
}

/// NyxID exposes no supported key-list pagination, so one response is the
/// completeness unit. Refuse oversized accounts visibly instead of buffering
/// an arbitrary response or describing an unbounded listing as a bounded pass.
const PROVIDER_KEY_LIST_MAX_BYTES: usize = 1024 * 1024;
const PROVIDER_KEY_LIST_MAX_ROWS: usize = 1_000;

#[derive(Debug, thiserror::Error)]
pub(super) enum ProviderKeyListingError {
    #[error("provider key listing request failed")]
    Request(#[from] reqwest::Error),
    #[error("provider key listing exceeded the {limit}-byte response limit")]
    TooManyBytes { limit: usize },
    #[error("provider key listing exceeded the {limit}-row limit")]
    TooManyRows { limit: usize },
    #[error("provider key listing response was not valid JSON")]
    InvalidJson(#[from] serde_json::Error),
}

/// A key as the platform describes it.
#[derive(Debug)]
pub struct MintedKey {
    /// The platform's identifier for the key, used to take it back.
    pub id: String,
    /// The value itself, returned only at creation and never again.
    pub full_key: String,
    /// The exact provider expiry CMA requested for this key.
    pub expires_at: SystemTime,
    /// NyxID's exact state version for the created key.
    pub state_version: i64,
    /// Hash of the normalized authority NyxID says the key actually received.
    pub grant_fingerprint: String,
}

/// NyxID's create response omits `expires_at`, even when it accepted and
/// persisted one. Keep the wire shape separate so CMA retains the exact instant
/// it sent instead of inventing a value from the response.
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
    let mut digest = sha2::Sha256::new();
    use sha2::Digest as _;
    digest.update(b"cma-nyxid-agent-key-grant-v1\0");
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
    Some(
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn minted_key_from_response(
    key: MintedKeyResponse,
    expected_service_ids: &[String],
    expires_at: SystemTime,
) -> Option<MintedKey> {
    let expected = normalized_identifiers(expected_service_ids)?;
    let returned = normalized_identifiers(&key.allowed_service_ids)?;
    if key.id.trim().is_empty()
        || key.full_key.trim().is_empty()
        || key.scopes != KEY_SCOPES
        || key.state_version <= 0
        || returned != expected
        || !key.allowed_node_ids.is_empty()
        || key.allow_all_services
        || key.allow_all_nodes
    {
        return None;
    }
    let grant_fingerprint = provider_grant_fingerprint(&key)?;
    Some(MintedKey {
        id: key.id,
        full_key: key.full_key,
        expires_at,
        state_version: key.state_version,
        grant_fingerprint,
    })
}

/// A service as the platform lists it.
#[derive(Debug, Deserialize)]
struct ServiceRow {
    id: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// Whether this owner's connection to the service is usable.
    ///
    /// Only the per-owner listing carries it; the catalogue does not, and an
    /// absent value is read as usable so the catalogue is unaffected. A key
    /// minted with reach to an inactive connection would fail at first use,
    /// which is a failure nobody could interpret.
    #[serde(default)]
    is_active: Option<bool>,
}

struct ServiceResolution {
    allowed: Vec<String>,
    complete: bool,
}

/// The name a key carries, which is how it is found again.
///
/// Both halves matter. The deployment name keeps one control plane from
/// reclaiming another's keys on a platform account they share; the session
/// identifier is what says whose it is once this process has forgotten.
pub fn key_name(deployment: &str, session: &SessionId) -> String {
    format!("{KEY_NAME_PREFIX}:{deployment}:{}", session.0)
}

/// The session a key was minted for, if this deployment minted it.
pub fn session_of(deployment: &str, name: &str) -> Option<SessionId> {
    [KEY_NAME_PREFIX, LEGACY_KEY_NAME_PREFIX]
        .into_iter()
        .find_map(|product| {
            let prefix = format!("{product}:{deployment}:");
            name.strip_prefix(&prefix)
                .map(|session| SessionId(session.to_string()))
        })
}

fn mint_request_body(
    deployment: &str,
    session: &SessionId,
    allowed_service_ids: &[String],
    expires_at: SystemTime,
) -> serde_json::Value {
    let expires_at = mongodb::bson::DateTime::from_system_time(expires_at)
        .try_to_rfc3339_string()
        .expect("a thirty-day API-key expiry is representable as RFC 3339");
    serde_json::json!({
        "name": key_name(deployment, session),
        "scopes": KEY_SCOPES,
        "expires_at": expires_at,
        "allowed_service_ids": allowed_service_ids,
        // Both stated rather than left out. The platform allows everything by
        // default, so omission would silently turn a scoped key into an open one.
        "allow_all_services": false,
        "allow_all_nodes": false,
    })
}

/// Mints and reclaims per-session credentials on the identity platform.
pub struct KeyIssuer {
    http: reqwest::Client,
    /// Root of the platform, as this process reaches it.
    base_url: String,
    /// What this deployment calls itself on the keys it mints.
    deployment: String,
}

impl KeyIssuer {
    /// Build an issuer.
    pub fn new(
        http: reqwest::Client,
        base_url: impl Into<String>,
        deployment: impl Into<String>,
    ) -> Self {
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            deployment: deployment.into(),
        }
    }

    /// Turn the names a profile carries into the platform's own identifiers.
    ///
    /// A profile names services the way a person selected them. Ordinary
    /// sessions may narrow when an optional service no longer resolves;
    /// deterministic Trigger sessions consume `complete` and fail closed.
    async fn resolve(
        &self,
        owner_token: &str,
        wanted: &[ServiceId],
    ) -> Result<ServiceResolution, ()> {
        if wanted.is_empty() {
            return Ok(ServiceResolution {
                allowed: Vec::new(),
                complete: true,
            });
        }
        let url = format!("{}{USER_SERVICES_PATH}", self.base_url);
        let rows: Vec<ServiceRow> = match self
            .http
            .get(&url)
            .bearer_auth(owner_token)
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(resp) => match resp.json::<ServiceListing>().await {
                Ok(listing) => listing.into_rows(),
                Err(e) => {
                    tracing::warn!(error = %e, "service listing was not usable");
                    return Err(());
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "could not list the owner's services");
                return Err(());
            }
        };

        let mut resolved = Vec::new();
        let mut complete = true;
        for want in wanted {
            let found = rows
                .iter()
                .find(|r| r.id == want.0 || r.slug.as_deref() == Some(want.0.as_str()));
            match found {
                Some(row) if row.is_active != Some(false) => {
                    if !resolved.contains(&row.id) {
                        resolved.push(row.id.clone());
                    }
                    // `/api/v1/user-services` guarantees this field. Keep
                    // legacy narrowing available to ordinary sessions when an
                    // older provider omits it, but absence is not positive
                    // evidence for Trigger's exact-service admission.
                    if row.is_active != Some(true) {
                        complete = false;
                        tracing::warn!(
                            service = %want,
                            "the owner service listing omitted active-state evidence"
                        );
                    }
                }
                // Named but absent. Logged because a selection silently doing
                // nothing is how somebody spends an afternoon wondering why
                // their session cannot reach something they ticked.
                Some(_) | None => {
                    complete = false;
                    tracing::warn!(service = %want, "this owner has no such active service");
                }
            }
        }
        Ok(ServiceResolution {
            allowed: resolved,
            complete,
        })
    }

    /// Read-only exact-service admission used before a Trigger session exists.
    pub(crate) async fn resolves_all(
        &self,
        owner_token: &str,
        wanted: &[ServiceId],
    ) -> Result<bool, layer2_core::StoreError> {
        self.resolve(owner_token, wanted)
            .await
            .map(|resolution| resolution.complete)
            .map_err(|()| {
                layer2_core::StoreError::Unavailable(
                    "the owner service listing is unavailable".into(),
                )
            })
    }

    /// Mint a credential for one session, reaching only the given services.
    ///
    /// `None` when the platform refuses, which leaves the session without a
    /// credential rather than with an unscoped one. A session that cannot act
    /// on its owner's behalf is visibly broken; one acting with more reach
    /// than its profile allows is not, and that is the worse failure.
    pub async fn mint(
        &self,
        session: &SessionId,
        owner: &SubjectId,
        owner_token: &str,
        services: &[ServiceId],
    ) -> Option<MintedKey> {
        let resolution = match self.resolve(owner_token, services).await {
            Ok(resolution) => resolution,
            Err(()) => return None,
        };
        if session.is_trigger_managed() && !resolution.complete {
            tracing::warn!(
                session_id = %session,
                %owner,
                "a required Trigger service did not resolve; not minting a degraded credential"
            );
            return None;
        }
        let allowed = resolution.allowed;
        if allowed.is_empty() {
            tracing::warn!(
                session_id = %session,
                %owner,
                "no service this session may reach resolved; not minting a credential"
            );
            return None;
        }

        let expires_at = session_key_expiry(SystemTime::now());
        let body = mint_request_body(&self.deployment, session, &allowed, expires_at);

        let url = format!("{}{KEYS_PATH}", self.base_url);
        match self
            .http
            .post(&url)
            .bearer_auth(owner_token)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<MintedKeyResponse>().await {
                    Ok(key) => {
                        // The value is never logged, only the fact and the reach.
                        let minted = minted_key_from_response(key, &allowed, expires_at);
                        if minted.is_some() {
                            tracing::info!(
                                session_id = %session,
                                services = allowed.len(),
                                "minted a credential for this session"
                            );
                        } else {
                            tracing::error!(
                                session_id = %session,
                                "mint response authority did not match the requested grant"
                            );
                        }
                        minted
                    }
                    Err(e) => {
                        tracing::error!(session_id = %session, error = %e, "mint response was not usable");
                        None
                    }
                }
            }
            Ok(resp) => {
                tracing::error!(session_id = %session, status = %resp.status(), "the platform refused to mint");
                None
            }
            Err(e) => {
                tracing::error!(session_id = %session, error = %e, "could not reach the platform to mint");
                None
            }
        }
    }

    /// Every service this owner has, for a caller choosing among them.
    ///
    /// Answered with the owner's own token, because which services exist is a
    /// fact about their account and this service's identity would answer with
    /// its own. Selections are stored by slug rather than identifier: a slug
    /// is what a person recognises, and it survives a service being rebuilt.
    pub async fn services_of(
        &self,
        owner_token: &str,
    ) -> Result<Vec<layer2_core::CatalogService>, layer2_core::CatalogError> {
        let url = format!("{}{SERVICES_PATH}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(owner_token)
            .send()
            .await
            .map_err(|e| layer2_core::CatalogError::Unavailable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(layer2_core::CatalogError::Unavailable(format!(
                "the platform answered {} listing services",
                resp.status()
            )));
        }
        let listing: ServiceListing = resp
            .json()
            .await
            .map_err(|e| layer2_core::CatalogError::Unavailable(e.to_string()))?;
        Ok(listing
            .into_rows()
            .into_iter()
            .map(|r| {
                // Named by slug where there is one, so a stored selection reads
                // as something a person chose rather than as an identifier.
                let id = r.slug.clone().unwrap_or_else(|| r.id.clone());
                layer2_core::CatalogService {
                    name: r.name.unwrap_or_else(|| id.clone()),
                    id: ServiceId::new(id),
                    description: r.description,
                }
            })
            .collect())
    }

    /// Take a key back.
    ///
    /// Failure is logged and not retried **here**, which is a statement about
    /// this function and not about the consequence: a reclaim that did not
    /// happen costs an open credential until its provider expiry rather than a
    /// harmless row in a list. The orphan sweep sees a surviving key again by
    /// re-listing it; the durable-debt sweep sees it again because it remains
    /// queued until this returns success. Every non-success response remains a
    /// failure: this client has no verified provider contract under which a 404
    /// proves that the intended credential was deleted. What must not happen is
    /// this returning quietly as though a possibly live credential were no
    /// longer owed.
    pub async fn revoke(&self, id: &str, owner_token: &str) -> bool {
        let url = format!("{}{KEYS_PATH}/{id}", self.base_url);
        match self.http.delete(&url).bearer_auth(owner_token).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!("reclaimed a session credential");
                true
            }
            Ok(resp) => {
                tracing::warn!(status = %resp.status(), "could not reclaim a session credential");
                false
            }
            Err(_) => {
                // The request error contains the URL, whose last component is
                // the provider key identifier. Keep that value out of logs.
                tracing::warn!("could not reach the platform to reclaim a session credential");
                false
            }
        }
    }

    /// Every key this deployment minted on an owner's account.
    ///
    /// Used to find the ones no session accounts for. Reads the name, because
    /// the name is the only record that survives this process restarting.
    ///
    /// NyxID 0.7.0 was driven on 2026-08-23 with
    /// `nyxid api-key list --output json`: it returned one `keys` envelope and
    /// no pagination metadata. Its matching server handler accepts no page
    /// query and collects the complete database cursor. This therefore reads
    /// one complete response rather than inventing an unsupported page shape.
    pub(super) async fn minted_here(
        &self,
        owner_token: &str,
    ) -> Result<Vec<(String, SessionId)>, ProviderKeyListingError> {
        let url = format!("{}{KEYS_PATH}", self.base_url);
        let response = self
            .http
            .get(&url)
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

        let listing: KeyListing = serde_json::from_slice(&body)?;
        let rows = listing.into_rows();
        if rows.len() > PROVIDER_KEY_LIST_MAX_ROWS {
            return Err(ProviderKeyListingError::TooManyRows {
                limit: PROVIDER_KEY_LIST_MAX_ROWS,
            });
        }
        Ok(rows
            .into_iter()
            .filter_map(|k| session_of(&self.deployment, &k.name).map(|s| (k.id, s)))
            .collect())
    }
}

/// A key row as the platform lists it.
#[derive(Debug, Deserialize)]
struct KeyRow {
    id: String,
    #[serde(default)]
    name: String,
}

/// The platform answers either a bare array or an object wrapping one.
///
/// Accepting both because which it is is not this service's decision, and a
/// listing that fails to parse would silently stop every reclaim. The wrapped
/// shape is deliberately closed: if NyxID later advertises pagination or
/// another completeness contract, that must fail rather than being ignored as
/// though the first response were still the whole account.
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
            Self::Bare(v) => v,
            Self::Wrapped(listing) => listing.keys,
        }
    }
}

/// The same shape question for services.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ServiceListing {
    Bare(Vec<ServiceRow>),
    Wrapped {
        #[serde(alias = "services", alias = "items")]
        keys: Vec<ServiceRow>,
    },
}

impl ServiceListing {
    fn into_rows(self) -> Vec<ServiceRow> {
        match self {
            Self::Bare(v) => v,
            Self::Wrapped { keys } => keys,
        }
    }
}

/// The service picker, answered as the caller.
///
/// Wraps the issuer because both ask the same platform the same question: the
/// picker to show what may be chosen, the mint to turn a choice into reach.
/// One place that knows how services are listed is one place to correct.
pub struct ServicePicker {
    issuer: std::sync::Arc<KeyIssuer>,
}

impl ServicePicker {
    /// Build a picker over an issuer.
    pub fn new(issuer: std::sync::Arc<KeyIssuer>) -> Self {
        Self { issuer }
    }
}

#[async_trait::async_trait]
impl layer2_core::ServiceCatalog for ServicePicker {
    async fn available(
        &self,
        _subject: &SubjectId,
        credential: Option<&str>,
    ) -> Result<Vec<layer2_core::CatalogService>, layer2_core::CatalogError> {
        // Only the caller's own credential will do. This service's identity
        // would answer with the services *it* has, which is a different
        // account and a misleading list rather than an empty one.
        let Some(token) = credential else {
            return Err(layer2_core::CatalogError::Unavailable(
                "no delegated credential to list this owner's services with".into(),
            ));
        };
        self.issuer.services_of(token).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use axum::{
        extract::{Path, State},
        http::{header::AUTHORIZATION, HeaderMap, StatusCode},
        response::IntoResponse,
        routing::{delete, get, post},
        Json, Router,
    };

    use super::*;

    fn valid_mint_response() -> MintedKeyResponse {
        MintedKeyResponse {
            id: "provider-key-1".into(),
            full_key: "secret-value".into(),
            scopes: KEY_SCOPES.into(),
            state_version: 1,
            allowed_service_ids: vec!["service-b".into(), "service-a".into()],
            allowed_node_ids: Vec::new(),
            allow_all_services: false,
            allow_all_nodes: false,
        }
    }

    #[test]
    fn mint_accepts_only_the_exact_returned_provider_grant() {
        let expires_at = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let minted = minted_key_from_response(
            valid_mint_response(),
            &["service-a".into(), "service-b".into()],
            expires_at,
        )
        .expect("the normalized returned grant matches exactly");

        assert_eq!(minted.state_version, 1);
        assert_eq!(minted.expires_at, expires_at);
        assert_eq!(minted.grant_fingerprint.len(), 64);
    }

    #[test]
    fn mint_rejects_ambient_or_malformed_returned_authority() {
        let expires_at = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let expected = ["service-a".into(), "service-b".into()];
        let mut invalid = Vec::new();

        let mut response = valid_mint_response();
        response.scopes = "proxy read write".into();
        invalid.push(response);
        let mut response = valid_mint_response();
        response.state_version = 0;
        invalid.push(response);
        let mut response = valid_mint_response();
        response.allowed_service_ids.push("service-a".into());
        invalid.push(response);
        let mut response = valid_mint_response();
        response.allowed_service_ids = vec!["service-a".into()];
        invalid.push(response);
        let mut response = valid_mint_response();
        response.allowed_node_ids = vec!["node-1".into()];
        invalid.push(response);
        let mut response = valid_mint_response();
        response.allow_all_services = true;
        invalid.push(response);
        let mut response = valid_mint_response();
        response.allow_all_nodes = true;
        invalid.push(response);
        let mut response = valid_mint_response();
        response.id.clear();
        invalid.push(response);
        let mut response = valid_mint_response();
        response.full_key.clear();
        invalid.push(response);

        for response in invalid {
            assert!(
                minted_key_from_response(response, &expected, expires_at).is_none(),
                "unsafe returned authority must fail closed"
            );
        }
    }

    #[derive(Clone)]
    struct ListingResponse {
        status: StatusCode,
        body: Arc<serde_json::Value>,
    }

    async fn list_keys(
        State(response): State<ListingResponse>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer owner-access")
        );
        (response.status, Json((*response.body).clone()))
    }

    async fn platform(
        status: StatusCode,
        body: serde_json::Value,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/api/v1/api-keys", get(list_keys))
            .with_state(ListingResponse {
                status,
                body: Arc::new(body),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock identity platform");
        let base = format!("http://{}", listener.local_addr().expect("mock address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock identity platform");
        });
        (base, server)
    }

    #[derive(Clone)]
    struct ServiceMintPlatform {
        services: Arc<serde_json::Value>,
        mints: Arc<AtomicUsize>,
    }

    async fn list_owner_services(
        State(platform): State<ServiceMintPlatform>,
        headers: HeaderMap,
    ) -> Json<serde_json::Value> {
        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer owner-access")
        );
        Json((*platform.services).clone())
    }

    async fn mint_scoped_key(
        State(platform): State<ServiceMintPlatform>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer owner-access")
        );
        platform.mints.fetch_add(1, Ordering::SeqCst);
        Json(serde_json::json!({
            "id": "provider-key-1",
            "full_key": "session-key-value",
            "scopes": KEY_SCOPES,
            "state_version": 1,
            "allowed_service_ids": body["allowed_service_ids"],
            "allowed_node_ids": [],
            "allow_all_services": false,
            "allow_all_nodes": false,
        }))
    }

    async fn service_mint_platform(
        services: serde_json::Value,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let mints = Arc::new(AtomicUsize::new(0));
        let state = ServiceMintPlatform {
            services: Arc::new(services),
            mints: Arc::clone(&mints),
        };
        let app = Router::new()
            .route("/api/v1/user-services", get(list_owner_services))
            .route("/api/v1/api-keys", post(mint_scoped_key))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock identity platform");
        let base = format!("http://{}", listener.local_addr().expect("mock address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock identity platform");
        });
        (base, mints, server)
    }

    async fn delete_key(
        State(status): State<StatusCode>,
        Path(id): Path<String>,
        headers: HeaderMap,
    ) -> StatusCode {
        assert_eq!(id, "key-under-test");
        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer owner-access")
        );
        status
    }

    async fn revocation_platform(status: StatusCode) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/api/v1/api-keys/{id}", delete(delete_key))
            .with_state(status);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock identity platform");
        let base = format!("http://{}", listener.local_addr().expect("mock address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock identity platform");
        });
        (base, server)
    }

    async fn closed_local_url() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("reserve a local address");
        let url = format!(
            "http://{}",
            listener.local_addr().expect("reserved address")
        );
        drop(listener);
        url
    }

    fn issuer(base: &str) -> KeyIssuer {
        KeyIssuer::new(reqwest::Client::new(), base, "production")
    }

    #[tokio::test]
    async fn a_successful_revoke_reports_success() {
        let (base, server) = revocation_platform(StatusCode::NO_CONTENT).await;

        assert!(issuer(&base).revoke("key-under-test", "owner-access").await);
        server.abort();
    }

    #[tokio::test]
    async fn a_not_found_revoke_reports_failure() {
        let (base, server) = revocation_platform(StatusCode::NOT_FOUND).await;

        assert!(!issuer(&base).revoke("key-under-test", "owner-access").await);
        server.abort();
    }

    #[tokio::test]
    async fn a_refused_revoke_reports_failure() {
        let (base, server) = revocation_platform(StatusCode::SERVICE_UNAVAILABLE).await;

        assert!(!issuer(&base).revoke("key-under-test", "owner-access").await);
        server.abort();
    }

    #[tokio::test]
    async fn an_unreachable_revoke_reports_failure() {
        let base = closed_local_url().await;

        assert!(!issuer(&base).revoke("key-under-test", "owner-access").await);
    }

    #[tokio::test]
    async fn trigger_mint_refuses_a_partial_service_resolution_while_ordinary_mint_narrows() {
        let (base, mints, server) = service_mint_platform(serde_json::json!({
            "services": [
                { "id": "model-service", "slug": "chrono-llm", "is_active": true },
                { "id": "github-service", "slug": "api-github", "is_active": false }
            ]
        }))
        .await;
        let issuer = issuer(&base);
        let wanted = [ServiceId::new("chrono-llm"), ServiceId::new("api-github")];

        assert!(!issuer
            .resolves_all("owner-access", &wanted)
            .await
            .expect("the complete listing is readable"));
        assert!(
            issuer
                .mint(
                    &SessionId("sesn_trg_0123456789abcdef".into()),
                    &SubjectId("owner".into()),
                    "owner-access",
                    &wanted,
                )
                .await
                .is_none(),
            "a Trigger key must never silently omit its selected GitHub service"
        );
        assert_eq!(mints.load(Ordering::SeqCst), 0);

        let ordinary = issuer
            .mint(
                &SessionId("sesn_ordinary".into()),
                &SubjectId("owner".into()),
                "owner-access",
                &wanted,
            )
            .await
            .expect("ordinary sessions retain the established narrowing policy");
        assert_eq!(ordinary.state_version, 1);
        assert_eq!(mints.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn trigger_resolution_requires_explicit_active_state_evidence() {
        let (base, mints, server) = service_mint_platform(serde_json::json!({
            "services": [
                { "id": "github-service", "slug": "api-github" }
            ]
        }))
        .await;
        let issuer = issuer(&base);
        let wanted = [ServiceId::new("github-service")];

        assert!(
            !issuer
                .resolves_all("owner-access", &wanted)
                .await
                .expect("the incomplete listing is readable"),
            "missing active-state evidence cannot admit a Trigger session"
        );
        assert!(
            issuer
                .mint(
                    &SessionId("sesn_trg_0123456789abcdef".into()),
                    &SubjectId("owner".into()),
                    "owner-access",
                    &wanted,
                )
                .await
                .is_none(),
            "Trigger minting must repeat the same fail-closed proof"
        );
        assert_eq!(mints.load(Ordering::SeqCst), 0);

        assert!(
            issuer
                .mint(
                    &SessionId("sesn_ordinary".into()),
                    &SubjectId("owner".into()),
                    "owner-access",
                    &wanted,
                )
                .await
                .is_some(),
            "ordinary sessions retain compatibility with legacy listings"
        );
        assert_eq!(mints.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[test]
    fn the_mint_body_has_exactly_the_demonstrated_expiring_field_set() {
        assert_eq!(
            mint_request_body(
                "production",
                &SessionId("sesn_abc".into()),
                &["user-service-1".into(), "user-service-2".into()],
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_000),
            ),
            serde_json::json!({
                "name": "cma:production:sesn_abc",
                "scopes": "proxy read",
                "expires_at": "2030-03-17T17:46:40Z",
                "allowed_service_ids": ["user-service-1", "user-service-2"],
                "allow_all_services": false,
                "allow_all_nodes": false,
            })
        );
    }

    #[test]
    fn a_new_key_carries_the_deployment_and_the_session_it_belongs_to() {
        let name = key_name("chrono-bot-prod", &SessionId("sesn_abc".into()));
        assert_eq!(name, "cma:chrono-bot-prod:sesn_abc");
        assert_eq!(
            session_of("chrono-bot-prod", &name),
            Some(SessionId("sesn_abc".into()))
        );
    }

    #[test]
    fn a_legacy_key_still_belongs_to_the_session_it_names() {
        assert_eq!(
            session_of("chrono-bot-prod", "chrono-bot:chrono-bot-prod:sesn_abc"),
            Some(SessionId("sesn_abc".into()))
        );
    }

    #[test]
    fn another_deployments_keys_are_not_ours_to_reclaim() {
        // The platform account is shared. Reading a name loosely would have one
        // control plane deleting credentials out from under another's sessions.
        let theirs = key_name("chrono-bot-staging", &SessionId("sesn_abc".into()));
        assert_eq!(session_of("chrono-bot-prod", &theirs), None);
        assert_eq!(
            session_of("chrono-bot-prod", "chrono-bot:chrono-bot-staging:sesn_abc"),
            None
        );
    }

    #[test]
    fn a_key_nobody_here_minted_is_left_alone() {
        // FKST mints per-session keys on the same accounts. Ours are the ones
        // that say so.
        assert_eq!(session_of("chrono-bot-prod", "fkst-session-0a676a08"), None);
        assert_eq!(session_of("chrono-bot-prod", "CI Pipeline Key"), None);
    }

    #[test]
    fn a_listing_parses_whether_or_not_it_is_wrapped() {
        let bare: KeyListing = serde_json::from_str(r#"[{"id":"k1","name":"n"}]"#).expect("bare");
        assert_eq!(bare.into_rows().len(), 1);
        let wrapped: KeyListing =
            serde_json::from_str(r#"{"keys":[{"id":"k1","name":"n"}]}"#).expect("wrapped");
        assert_eq!(wrapped.into_rows().len(), 1);
    }

    #[test]
    fn an_unimplemented_pagination_contract_is_not_silently_ignored() {
        let result = serde_json::from_str::<KeyListing>(
            r#"{"keys":[{"id":"k1","name":"n"}],"pagination":{"hasNextPage":true}}"#,
        );

        assert!(
            result.is_err(),
            "a response that may be partial must not parse as a complete listing"
        );
    }

    #[tokio::test]
    async fn a_complete_unpaginated_listing_is_not_cut_to_an_assumed_page_size() {
        let keys = (0..125)
            .map(|number| {
                serde_json::json!({
                    "id": format!("key-{number}"),
                    "name": key_name(
                        "production",
                        &SessionId(format!("sesn_{number}")),
                    ),
                })
            })
            .collect::<Vec<_>>();
        let (base, server) = platform(StatusCode::OK, serde_json::json!({ "keys": keys })).await;

        let listed = issuer(&base)
            .minted_here("owner-access")
            .await
            .expect("complete key listing");

        assert_eq!(listed.len(), 125);
        assert_eq!(listed[124].1, SessionId("sesn_124".into()));
        server.abort();
    }

    #[tokio::test]
    async fn an_oversized_unpaginated_key_row_set_fails_visibly() {
        let keys = (0..=PROVIDER_KEY_LIST_MAX_ROWS)
            .map(|number| {
                serde_json::json!({
                    "id": format!("k{number}"),
                    "name": format!("foreign-{number}"),
                })
            })
            .collect::<Vec<_>>();
        let (base, server) = platform(StatusCode::OK, serde_json::json!({ "keys": keys })).await;

        let result = issuer(&base).minted_here("owner-access").await;

        assert!(matches!(
            result,
            Err(ProviderKeyListingError::TooManyRows {
                limit: PROVIDER_KEY_LIST_MAX_ROWS
            })
        ));
        server.abort();
    }

    #[tokio::test]
    async fn an_oversized_unpaginated_key_response_fails_before_json_buffering() {
        let oversized_name = "x".repeat(PROVIDER_KEY_LIST_MAX_BYTES + 1);
        let body = serde_json::json!({
            "keys": [{ "id": "k", "name": oversized_name }]
        });
        let (base, server) = platform(StatusCode::OK, body).await;

        let result = issuer(&base).minted_here("owner-access").await;

        assert!(matches!(
            result,
            Err(ProviderKeyListingError::TooManyBytes {
                limit: PROVIDER_KEY_LIST_MAX_BYTES
            })
        ));
        server.abort();
    }

    #[tokio::test]
    async fn a_listing_correlates_both_generations_of_key_name() {
        let keys = serde_json::json!({
            "keys": [
                {
                    "id": "new-key",
                    "name": key_name("production", &SessionId("sesn_new".into()))
                },
                {
                    "id": "legacy-key",
                    "name": "chrono-bot:production:sesn_legacy"
                },
                {
                    "id": "another-deployment",
                    "name": "chrono-bot:staging:sesn_legacy"
                }
            ]
        });
        let (base, server) = platform(StatusCode::OK, keys).await;

        let listed = issuer(&base)
            .minted_here("owner-access")
            .await
            .expect("complete key listing");

        assert_eq!(
            listed,
            vec![
                ("new-key".into(), SessionId("sesn_new".into())),
                ("legacy-key".into(), SessionId("sesn_legacy".into())),
            ]
        );
        server.abort();
    }

    #[tokio::test]
    async fn a_failed_listing_is_distinct_from_an_empty_account() {
        let (empty_base, empty_server) =
            platform(StatusCode::OK, serde_json::json!({ "keys": [] })).await;
        let empty = issuer(&empty_base)
            .minted_here("owner-access")
            .await
            .expect("empty account is a complete listing");
        assert!(empty.is_empty());
        empty_server.abort();

        let (failed_base, failed_server) = platform(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({ "message": "listing unavailable" }),
        )
        .await;
        assert!(
            issuer(&failed_base)
                .minted_here("owner-access")
                .await
                .is_err(),
            "a failed request must not read as an empty account"
        );
        failed_server.abort();
    }

    #[test]
    fn the_mint_reads_the_owners_own_services_and_the_picker_reads_the_catalogue() {
        // These are two different id spaces for the same service, and confusing
        // them is why minting never once succeeded in production: every call
        // was refused with `UserService '…' not found or not permitted`, for
        // every profile, from the day the feature shipped.
        //
        // Pinned as constants rather than as behaviour because the failure was
        // silent — `mint` returns `None`, the session is left without a
        // credential, and the sweep retries every five minutes forever.
        assert_eq!(
            USER_SERVICES_PATH, "/api/v1/user-services",
            "`allowed_service_ids` means UserService identifiers, which only this route returns"
        );
        assert_eq!(
            SERVICES_PATH, "/api/v1/services",
            "the picker needs names and descriptions, which only the catalogue carries"
        );
        assert_ne!(
            SERVICES_PATH, USER_SERVICES_PATH,
            "one route cannot answer both questions"
        );
    }

    #[test]
    fn a_per_owner_row_is_readable_without_the_fields_only_the_catalogue_has() {
        // The owner's listing carries neither `name` nor `description` — it has
        // `label` and `catalog_service_name` instead — so a reader that
        // required them would drop every row and mint nothing, which reads
        // exactly like the bug being fixed.
        let listing: ServiceListing = serde_json::from_str(
            r#"{"services":[{"id":"5a8b8724","slug":"chrono-llm","is_active":true}]}"#,
        )
        .expect("the owner's own listing");
        let rows = listing.into_rows();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "5a8b8724");
        assert_eq!(rows[0].slug.as_deref(), Some("chrono-llm"));
        assert_eq!(rows[0].is_active, Some(true));
    }

    #[test]
    fn a_catalogue_row_still_reads_although_it_names_no_activity() {
        // `is_active` belongs to a connection, not to a service, so the
        // catalogue omits it. Absent must read as usable or the picker empties.
        let listing: ServiceListing = serde_json::from_str(
            r#"{"services":[{"id":"90b07c84","slug":"llm-anthropic","name":"Anthropic API","description":"d"}]}"#,
        )
        .expect("the catalogue");
        let rows = listing.into_rows();

        assert_eq!(rows[0].is_active, None);
        assert_eq!(rows[0].name.as_deref(), Some("Anthropic API"));
    }
}
