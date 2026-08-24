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
//! **There is no backstop. Deletion is the only hygiene there is.** This module
//! used to say the opposite — that these keys last a day whatever happens to
//! them, so one left behind stops working on its own — and every decision
//! about tolerating a failed reclaim was justified by it. The mint below
//! requests no expiry, and production reports `expires_at: null` on every key
//! this service has ever created. The claim was never true.
//!
//! It is worth being blunt about the cost, because four separate comments in
//! this tree leaned on it: a key we fail to take back is a live credential
//! carrying `proxy read` over that session's services, for ever, held by
//! nobody and watched by nobody. Reclaiming needs its owner's token, which
//! this process may not hold after a restart until they next sign in, and
//! until then the key stays valid.
//!
//! Giving these keys a bounded life is #445's own follow-up. Until that is
//! demonstrated against the platform rather than assumed, nothing here may be
//! written as though it already were.

use layer2_core::{ServiceId, SessionId, SubjectId};
use serde::Deserialize;

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

/// A key as the platform describes it.
#[derive(Debug, Deserialize)]
pub struct MintedKey {
    /// The platform's identifier for the key, used to take it back.
    pub id: String,
    /// The value itself, returned only at creation and never again.
    pub full_key: String,
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
    /// A profile names services the way a person selected them. Anything that
    /// no longer resolves is dropped rather than failing the mint: a service
    /// removed from the account should narrow the session, not stop it.
    async fn resolve(&self, owner_token: &str, wanted: &[ServiceId]) -> Vec<String> {
        if wanted.is_empty() {
            return Vec::new();
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
                    return Vec::new();
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "could not list the owner's services");
                return Vec::new();
            }
        };

        let mut resolved = Vec::new();
        for want in wanted {
            let found = rows
                .iter()
                .filter(|r| r.is_active.unwrap_or(true))
                .find(|r| r.id == want.0 || r.slug.as_deref() == Some(want.0.as_str()));
            match found {
                Some(row) => {
                    if !resolved.contains(&row.id) {
                        resolved.push(row.id.clone());
                    }
                }
                // Named but absent. Logged because a selection silently doing
                // nothing is how somebody spends an afternoon wondering why
                // their session cannot reach something they ticked.
                None => tracing::warn!(service = %want, "this owner has no such service"),
            }
        }
        resolved
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
        let allowed = self.resolve(owner_token, services).await;
        if allowed.is_empty() {
            tracing::warn!(
                session_id = %session,
                %owner,
                "no service this session may reach resolved; not minting a credential"
            );
            return None;
        }

        let body = serde_json::json!({
            "name": key_name(&self.deployment, session),
            "scopes": KEY_SCOPES,
            "allowed_service_ids": allowed,
            // Both stated rather than left out. The platform allows everything
            // by default, so an omitted field here is the difference between
            // the feature working and the feature appearing to work.
            "allow_all_services": false,
            "allow_all_nodes": false,
        });

        let url = format!("{}{KEYS_PATH}", self.base_url);
        match self
            .http
            .post(&url)
            .bearer_auth(owner_token)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => match resp.json::<MintedKey>().await {
                Ok(key) => {
                    // The value is never logged, only the fact and the reach.
                    tracing::info!(
                        session_id = %session,
                        services = allowed.len(),
                        "minted a credential for this session"
                    );
                    Some(key)
                }
                Err(e) => {
                    tracing::error!(session_id = %session, error = %e, "mint response was not usable");
                    None
                }
            },
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
    /// this function and not about the consequence: these keys carry no
    /// expiry, so a reclaim that did not happen costs an open credential
    /// rather than a row in a list. The orphan sweep sees a surviving key again
    /// by re-listing it; the superseded sweep sees it again because it remains
    /// queued until this returns success. Every non-success response remains a
    /// failure: this client has no verified provider contract under which a 404
    /// proves that the intended credential was deleted. What must not happen is
    /// this returning quietly as though a possibly live credential were no
    /// longer owed.
    pub async fn revoke(&self, id: &str, owner_token: &str) -> bool {
        let url = format!("{}{KEYS_PATH}/{id}", self.base_url);
        match self.http.delete(&url).bearer_auth(owner_token).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!(key_id = %id, "reclaimed a session credential");
                true
            }
            Ok(resp) => {
                tracing::warn!(key_id = %id, status = %resp.status(), "could not reclaim a session credential");
                false
            }
            Err(e) => {
                tracing::warn!(key_id = %id, error = %e, "could not reach the platform to reclaim");
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
    pub async fn minted_here(
        &self,
        owner_token: &str,
    ) -> Result<Vec<(String, SessionId)>, reqwest::Error> {
        let url = format!("{}{KEYS_PATH}", self.base_url);
        let listing: KeyListing = self
            .http
            .get(&url)
            .bearer_auth(owner_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(listing
            .into_rows()
            .into_iter()
            .filter_map(|k| session_of(&self.deployment, &k.name).map(|s| (k.id, s)))
            .collect())
    }

    /// Every key this deployment named for one session on an owner's account.
    ///
    /// Kept over [`Self::minted_here`] rather than issuing a second listing
    /// request shape: pagination and platform response handling belong to that
    /// one path, whether a caller is sweeping an account or releasing one
    /// session after this process has forgotten what it minted.
    pub async fn minted_for(
        &self,
        session: &SessionId,
        owner_token: &str,
    ) -> Result<Vec<String>, reqwest::Error> {
        Ok(self
            .minted_here(owner_token)
            .await?
            .into_iter()
            .filter_map(|(id, named)| (named == *session).then_some(id))
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
    use std::sync::Arc;

    use axum::{
        extract::{Path, State},
        http::{header::AUTHORIZATION, HeaderMap, StatusCode},
        response::IntoResponse,
        routing::{delete, get},
        Json, Router,
    };

    use super::*;

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
