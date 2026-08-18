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
//! **Expiry is the backstop, deletion is the hygiene.** These keys last a day
//! whatever happens to them, so one left behind stops working on its own. That
//! is what makes it acceptable that reclaiming a key needs its owner's token,
//! which this process may not hold after a restart until they next sign in.

use layer2_core::{ServiceId, SessionId, SubjectId};
use serde::Deserialize;

/// Where keys are minted, relative to the platform root.
const KEYS_PATH: &str = "/api/v1/api-keys";

/// Where services are listed, relative to the platform root.
///
/// Needed because a profile names services the way a person reads them and the
/// minting call wants the platform's own identifiers.
const SERVICES_PATH: &str = "/api/v1/services";

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
}

/// The name a key carries, which is how it is found again.
///
/// Both halves matter. The deployment name keeps one control plane from
/// reclaiming another's keys on a platform account they share; the session
/// identifier is what says whose it is once this process has forgotten.
pub fn key_name(deployment: &str, session: &SessionId) -> String {
    format!("chrono-bot:{deployment}:{}", session.0)
}

/// The session a key was minted for, if this deployment minted it.
pub fn session_of(deployment: &str, name: &str) -> Option<SessionId> {
    let prefix = format!("chrono-bot:{deployment}:");
    name.strip_prefix(&prefix).map(|s| SessionId(s.to_string()))
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
        let url = format!("{}{SERVICES_PATH}", self.base_url);
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
    /// Failure is logged and not retried here: the key expires on its own, so
    /// a reclaim that did not happen costs a row in a list rather than an open
    /// credential.
    pub async fn revoke(&self, id: &str, owner_token: &str) {
        let url = format!("{}{KEYS_PATH}/{id}", self.base_url);
        match self.http.delete(&url).bearer_auth(owner_token).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!(key_id = %id, "reclaimed a session credential")
            }
            Ok(resp) => {
                tracing::warn!(key_id = %id, status = %resp.status(), "could not reclaim a session credential")
            }
            Err(e) => {
                tracing::warn!(key_id = %id, error = %e, "could not reach the platform to reclaim")
            }
        }
    }

    /// Every key this deployment minted on an owner's account.
    ///
    /// Used to find the ones no session accounts for. Reads the name, because
    /// the name is the only record that survives this process restarting.
    pub async fn minted_here(&self, owner_token: &str) -> Vec<(String, SessionId)> {
        let url = format!("{}{KEYS_PATH}", self.base_url);
        let listing: KeyListing = match self
            .http
            .get(&url)
            .bearer_auth(owner_token)
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(resp) => match resp.json().await {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(error = %e, "key listing was not usable");
                    return Vec::new();
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "could not list minted credentials");
                return Vec::new();
            }
        };
        listing
            .into_rows()
            .into_iter()
            .filter_map(|k| session_of(&self.deployment, &k.name).map(|s| (k.id, s)))
            .collect()
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
/// listing that fails to parse would silently stop every reclaim.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KeyListing {
    Bare(Vec<KeyRow>),
    Wrapped {
        #[serde(alias = "api_keys", alias = "items")]
        keys: Vec<KeyRow>,
    },
}

impl KeyListing {
    fn into_rows(self) -> Vec<KeyRow> {
        match self {
            Self::Bare(v) => v,
            Self::Wrapped { keys } => keys,
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
    use super::*;

    #[test]
    fn a_key_carries_the_deployment_and_the_session_it_belongs_to() {
        let name = key_name("chrono-bot-prod", &SessionId("sesn_abc".into()));
        assert_eq!(name, "chrono-bot:chrono-bot-prod:sesn_abc");
        assert_eq!(
            session_of("chrono-bot-prod", &name),
            Some(SessionId("sesn_abc".into()))
        );
    }

    #[test]
    fn another_deployments_keys_are_not_ours_to_reclaim() {
        // The platform account is shared. Reading a name loosely would have one
        // control plane deleting credentials out from under another's sessions.
        let theirs = key_name("chrono-bot-staging", &SessionId("sesn_abc".into()));
        assert_eq!(session_of("chrono-bot-prod", &theirs), None);
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
}
