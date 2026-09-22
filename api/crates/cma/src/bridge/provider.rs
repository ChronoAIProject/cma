use std::{
    collections::BTreeSet,
    time::{Duration, SystemTime},
};

use layer2_core::bridge::{BridgeConnector, BridgeError, BridgeRouteObservation, BridgeSetupBlock};
use layer2_core::resource_access::ResourceOwner;
use layer2_core::SubjectId;
use serde::de::DeserializeOwned;

use super::types::*;
use crate::auth::persistence::SecretValue;

mod relay;
mod setup;

const MAX_ROWS: usize = 1_000;
const DEADLINE: Duration = Duration::from_secs(5);

pub(crate) struct NyxBridgeProvider {
    base_url: reqwest::Url,
    http: reqwest::Client,
}

pub(crate) struct VerifiedProviderBinding {
    pub routes: Vec<BridgeRouteObservation>,
    pub key_state_version: i64,
    pub key_authority: KeyAuthority,
    pub deadline: tokio::time::Instant,
    pub scopes: String,
    pub allowed_service_ids: Vec<String>,
    pub allow_all_services: bool,
    pub allowed_node_ids: Vec<String>,
    pub allow_all_nodes: bool,
}

impl NyxBridgeProvider {
    /// Prove possession of the exact selected key with a read that NyxID fences
    /// against the authenticated API-key ID. Returned messages are ignored.
    pub async fn verify_secret(
        &self,
        owner: &ResourceOwner,
        secret: &SecretValue,
        connector: &BridgeConnector,
        human_subject: &SubjectId,
        human_credential: &str,
    ) -> Result<VerifiedProviderBinding, BridgeError> {
        if !raw_agent_key(secret.expose()) {
            return Err(binding_mismatch());
        }
        tokio::time::timeout(DEADLINE, async {
            let start = tokio::time::Instant::now();
            self.verify_human(human_subject, human_credential).await?;
            let binding = self
                .read_binding(
                    owner,
                    human_subject,
                    &SecretValue::new(human_credential.to_owned()),
                    connector,
                )
                .await?;
            let mut endpoint = self.base_url.clone();
            endpoint.set_path("/api/v1/channel-relay/messages");
            endpoint
                .path_segments_mut()
                .map_err(|_| binding_mismatch())?
                .push(&connector.route_ids[0]);
            endpoint
                .query_pairs_mut()
                .append_pair("page", "1")
                .append_pair("per_page", "1");
            let probe: BindingProbe = self.fetch(endpoint, secret).await?;
            if probe.page != 1
                || probe.per_page != 1
                || probe.messages.len() > 1
                || probe.total < probe.messages.len() as u64
            {
                return Err(binding_mismatch());
            }
            // Possession reads update NyxID's last_used_at and row version.
            // Recheck all current authority after that IO, within this same
            // timeout; a counter increase alone cannot identify its cause.
            let mut current = self
                .read_binding(
                    owner,
                    human_subject,
                    &SecretValue::new(human_credential.to_owned()),
                    connector,
                )
                .await?;
            if current.key_state_version < binding.key_state_version
                || current.key_authority != binding.key_authority
            {
                return Err(binding_mismatch());
            }
            current.deadline = current
                .deadline
                .min(binding.deadline)
                .min(start + Duration::from_secs(60));
            Ok(current)
        })
        .await
        .map_err(|_| BridgeError::AuthorityUnavailable)?
    }

    /// NyxID fences this route against API keys, service accounts, delegation
    /// and relay credentials. Its subject must remain the actual human actor.
    pub(crate) async fn verify_human(
        &self,
        subject: &SubjectId,
        credential: &str,
    ) -> Result<(), BridgeError> {
        if credential.starts_with("nyx_") || credential.starts_with("nyxid_") {
            return Err(super::control_plane::reconnect());
        }
        let mut endpoint = self.base_url.clone();
        endpoint.set_path("/api/v1/users/me/authorization");
        let account: Account = self
            .fetch(endpoint, &SecretValue::new(credential.to_owned()))
            .await?;
        if !account.is_active || account.id != subject.as_str() {
            return Err(super::control_plane::reconnect());
        }
        Ok(())
    }

    pub fn new(base_url: &str) -> Result<Self, BridgeError> {
        let base_url = reqwest::Url::parse(base_url)
            .map_err(|_| BridgeError::InvalidConfiguration("provider_url"))?;
        // This endpoint comes from deployment AuthMode, never a Bridge DTO.
        // Existing deployments may use HTTP to an internal identity service.
        if !matches!(base_url.scheme(), "https" | "http")
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || !matches!(base_url.path(), "" | "/")
        {
            return Err(BridgeError::InvalidConfiguration("provider_url"));
        }
        Self::with_endpoint(base_url)
    }

    fn with_endpoint(base_url: reqwest::Url) -> Result<Self, BridgeError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(DEADLINE)
            .build()
            .map_err(|_| BridgeError::AuthorityUnavailable)?;
        Ok(Self { base_url, http })
    }

    /// Use the exact supplied credential only. No NyxID mutation, token refresh,
    /// bot verification call (which can contact a real platform), or callback
    /// rewrite is reachable through this adapter.
    #[cfg(test)]
    pub async fn inspect(
        &self,
        owner: &ResourceOwner,
        credential_subject: &SubjectId,
        credential: &SecretValue,
        connector: &BridgeConnector,
    ) -> Result<VerifiedProviderBinding, BridgeError> {
        tokio::time::timeout(
            DEADLINE,
            self.read_binding(owner, credential_subject, credential, connector),
        )
        .await
        .map_err(|_| BridgeError::AuthorityUnavailable)?
    }

    async fn read_binding(
        &self,
        owner: &ResourceOwner,
        credential_subject: &SubjectId,
        credential: &SecretValue,
        connector: &BridgeConnector,
    ) -> Result<VerifiedProviderBinding, BridgeError> {
        if connector.base_url.trim_end_matches('/') != self.base_url.as_str().trim_end_matches('/')
            || connector.issuer.trim_end_matches('/')
                != self.base_url.as_str().trim_end_matches('/')
            || !canonical_id(owner_subject(owner).as_str())
            || !canonical_id(credential_subject.as_str())
            || !canonical_id(&connector.bot_id)
            || !canonical_id(&connector.key_id)
            || connector.route_ids.is_empty()
            || connector.route_ids.len() > 64
            || connector.route_ids.iter().any(|id| !canonical_id(id))
        {
            return Err(binding_mismatch());
        }
        if matches!(owner, ResourceOwner::Person { subject } if subject != credential_subject) {
            return Err(binding_mismatch());
        }
        let monotonic_start = tokio::time::Instant::now();
        let system_start = SystemTime::now();
        let (bots, keys, routes) = tokio::try_join!(
            self.get::<Bots>("/api/v1/channel-bots", owner, credential),
            self.get::<Keys>("/api/v1/api-keys", owner, credential),
            // All owner routes are required to detect a key used by another
            // bot or unselected route. Never filter this request by bot ID.
            self.get::<Routes>("/api/v1/channel-conversations", owner, credential),
        )?;
        require_complete(
            bots.bots.len(),
            Some(bots.total),
            bots.bots.iter().map(|b| b.id.as_str()),
        )?;
        require_complete(
            keys.keys.len(),
            None,
            keys.keys.iter().map(|k| k.authority.id.as_str()),
        )?;
        require_complete(
            routes.conversations.len(),
            Some(routes.total),
            routes.conversations.iter().map(|r| r.id.as_str()),
        )?;
        // List scope itself is owner authority, and every bot carries an exact
        // owner field. An unexpected row invalidates the complete observation.
        if bots
            .bots
            .iter()
            .any(|bot| bot.user_id != owner_subject(owner).as_str())
        {
            return Err(binding_mismatch());
        }
        for key in &keys.keys {
            key.authority
                .credential_source
                .require_owner(owner, credential_subject)?;
        }
        let bot = bots
            .bots
            .iter()
            .find(|bot| bot.id == connector.bot_id)
            .ok_or_else(binding_mismatch)?;
        let key = keys
            .keys
            .iter()
            .find(|key| key.authority.id == connector.key_id)
            .ok_or_else(binding_mismatch)?;
        let key_state_version = key.state_version;
        let key = &key.authority;
        if !bot.is_active
            || super::capability_catalog::supported_platform(&bot.platform).as_ref()
                != Some(&connector.platform)
            || !key.is_active
            || key_state_version <= 0
            || key.bindings_count != 0
            || key.platform.as_ref().is_some_and(|platform| {
                super::capability_catalog::supported_platform(platform).as_ref()
                    != Some(&connector.platform)
            })
        {
            return Err(binding_mismatch());
        }
        if key.callback_url.as_deref() != Some(connector.callback_url.as_str()) {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::CallbackConflict,
            ));
        }
        let mut deadline = monotonic_start + Duration::from_secs(60);
        if let Some(expiry) = &key.expires_at {
            let expiration = mongodb::bson::DateTime::parse_rfc3339_str(expiry)
                .map_err(|_| binding_mismatch())?
                .to_system_time();
            let remaining = expiration
                .duration_since(system_start)
                .map_err(|_| binding_mismatch())?;
            deadline = deadline.min(monotonic_start + remaining);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::AuthorityExpired,
            ));
        }
        let selected: BTreeSet<_> = connector.route_ids.iter().collect();
        if selected.len() != connector.route_ids.len() {
            return Err(binding_mismatch());
        }
        let platform = serde_json::to_value(&connector.platform).map_err(|_| binding_mismatch())?;
        let mut verified = Vec::new();
        for route in &routes.conversations {
            if route.is_active && route.agent_api_key_id == key.id && !selected.contains(&route.id)
            {
                return Err(BridgeError::ConnectionRequired(
                    BridgeSetupBlock::CallbackConflict,
                ));
            }
            if !selected.contains(&route.id) {
                continue;
            }
            if !route.is_active
                || route.agent_api_key_id != key.id
                || route.channel_bot_id.as_deref() != Some(bot.id.as_str())
                || Some(route.platform.as_str()) != platform.as_str()
            {
                return Err(binding_mismatch());
            }
            verified.push(BridgeRouteObservation {
                route_id: route.id.clone(),
                owner: owner.clone(),
                bot_id: bot.id.clone(),
                key_id: key.id.clone(),
                enabled: true,
            });
        }
        if verified.len() != selected.len() {
            return Err(binding_mismatch());
        }
        Ok(VerifiedProviderBinding {
            routes: verified,
            key_state_version,
            key_authority: key.clone(),
            deadline,
            scopes: key.scopes.clone(),
            allowed_service_ids: key.allowed_service_ids.clone(),
            allow_all_services: key.allow_all_services,
            allowed_node_ids: key.allowed_node_ids.clone(),
            allow_all_nodes: key.allow_all_nodes,
        })
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        owner: &ResourceOwner,
        credential: &SecretValue,
    ) -> Result<T, BridgeError> {
        let mut endpoint = self.base_url.clone();
        endpoint.set_path(path);
        if let ResourceOwner::Organization { subject } = owner {
            endpoint
                .query_pairs_mut()
                .append_pair("org_id", subject.as_str());
        }
        self.fetch(endpoint, credential).await
    }

    async fn fetch<T: DeserializeOwned>(
        &self,
        endpoint: reqwest::Url,
        credential: &SecretValue,
    ) -> Result<T, BridgeError> {
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(credential.expose())
            .send()
            .await
            .map_err(|_| BridgeError::AuthorityUnavailable)?;
        // Preserve the bridge's exact-status and fail-closed metadata contract.
        if response.status().as_u16() != 200 {
            return Err(match response.status().as_u16() {
                401 | 403 | 404 => binding_mismatch(),
                _ => BridgeError::AuthorityUnavailable,
            });
        }
        crate::auth::provider_http::read_json(response)
            .await
            .map_err(|error| match error {
                crate::auth::provider_http::ProviderReadError::Unavailable => {
                    BridgeError::AuthorityUnavailable
                }
                _ => binding_mismatch(),
            })
    }
}

fn raw_agent_key(value: &str) -> bool {
    // The retained-key formats generated by pinned NyxID crypto/token.rs and
    // key_service.rs. Relay/reply JWTs and human OAuth credentials cannot be
    // imported into long-lived bridge custody, even if an endpoint accepts them.
    value
        .strip_prefix("nyxid_ag_")
        .or_else(|| value.strip_prefix("nyx_"))
        .is_some_and(|suffix| {
            suffix.len() == 64
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

fn require_complete<'a>(
    length: usize,
    total: Option<u64>,
    ids: impl Iterator<Item = &'a str>,
) -> Result<(), BridgeError> {
    let unique: BTreeSet<_> = ids.collect();
    if length > MAX_ROWS
        || total.is_some_and(|total| total != length as u64)
        || unique.len() != length
        || unique.iter().any(|id| !canonical_id(id))
    {
        return Err(binding_mismatch());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
