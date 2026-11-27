//! Browser and bearer authentication against the configured identity provider.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::http::HeaderMap;
use layer2_core::{Authorization, Capability, StoreError, SubjectId};
use tokio::sync::RwLock;

pub mod account_standing;
pub mod actor_links;
mod agent_keys;
pub mod bearer;
mod bearer_authorization;
mod binding;
mod bots;
mod browser_login;
mod browser_origins;
mod browser_session;
mod cookie_authorization;
mod creation_outcomes;
pub mod deferred_commands;
mod desktop;
pub mod entitlements;
pub mod flow;
mod identity;
mod identity_cooldown;
mod identity_evidence;
mod identity_flight;
pub use crate::provider::identity_http;
mod identity_lifetime;
use crate::provider::identity_observations;
pub mod identity_retry;
use crate::provider::identity_throttle;
pub use identity_throttle::RetryAfter as ProviderRetryAfter;
#[allow(dead_code, reason = "the aggregate-neutral issuer is consumed by #809")]
pub mod keys;
pub mod middleware;
#[cfg(test)]
mod nyxid_contract_tests;
pub mod nyxid_proxy;
mod oauth_commands;
pub mod organizations;
mod permissions;
pub mod persistence;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support;
pub use crate::profile;
pub use crate::provider::provider_http;
mod provider_metadata;
#[cfg(test)]
mod public_contract_tests;
pub mod records;
mod refresh;
pub mod registration;
pub mod relay;
mod resource_request;
mod sandbox_prerequisite;
pub mod service_authorization;
pub mod service_portal;
pub mod session_routes;
mod signup_options;
mod silent_preference;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_resources;
pub use crate::token;
mod tokens;
mod usage_reporting;
pub mod verification_diagnostics;
pub use crate::provider::verification_error;
mod worker_credentials;

pub use flow::account_router;
pub use flow::router;
pub use middleware::attach_subject;
pub use records::SignInRecords;
pub use tokens::ProviderMetadata;

use persistence::{AuthStore, DurableSubjectTokens, SecretDigest, SubjectAuthState};
use profile::Profile;

pub const AUTH_COOKIE: &str = "cma_session";
pub const AUTH_COOKIE_DOMAIN: &str = "browser-login";
const AUTH_COOKIE_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
pub const SIGN_IN_PREFERENCE_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const LOGIN_ATTEMPT_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// One minute beyond the ten-minute attempt for callback I/O and response processing.
/// This margin extends browser custody only, never the attempt's authority.
pub const BROWSER_BINDING_LIFETIME: Duration =
    Duration::from_secs(LOGIN_ATTEMPT_LIFETIME.as_secs() + 60);
/// Time of explicit logout; compared with the start of a successful interactive login.
const SIGNED_OUT_COOKIE: &str = "cma_signed_out";

/// How CMA establishes the caller attached to an HTTP request.
#[derive(Clone)]
pub enum AuthMode {
    Provider {
        base_url: String,
        client_id: String,
        client_secret: String,
        public_url: String,
    },
    Trusted {
        subject: String,
    },
}

impl std::fmt::Debug for AuthMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider {
                base_url,
                client_id,
                public_url,
                ..
            } => formatter
                .debug_struct("Provider")
                .field("base_url", base_url)
                .field("client_id", client_id)
                .field("client_secret", &"<redacted>")
                .field("public_url", public_url)
                .finish(),
            Self::Trusted { subject } => formatter
                .debug_struct("Trusted")
                .field("subject", subject)
                .finish(),
        }
    }
}

pub struct AuthState {
    creation_outcomes_enabled: bool,
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
    account_deletion_enabled: bool,
    privacy_export_enabled: bool,
    binding_diagnostics_enabled: bool,
    platform_settings: Option<Arc<layer2_core::PlatformSettingsService>>,
    bot_father_username: Option<String>,
    agent_key_service_id: Option<String>,
    oauth_api_resource: Option<String>,
    resource_request_enabled: bool,
    silent_sign_in_enabled: bool,
    pub(super) legal_terms: Option<layer2_core::legal_consent::LegalTerms>,
    browser_session_join_enabled: bool,
    service_authorization: Option<Arc<dyn service_authorization::GrantAuthority>>,
    #[cfg(test)]
    preference_clock: fn() -> SystemTime,
    identity_proxy: Option<nyxid_proxy::jwt::Verifier>,
    proxy_replay_principal_enabled: bool,
    bindings: Option<Arc<layer2_core::sandbox::binding::BindingAccessService>>,
    mode: AuthMode,
    /// Browser-facing NyxID origin used for registration handoffs.
    pub(super) nyxid_public_url: String,
    browser_origins: Vec<String>,
    pub(super) http: reqwest::Client,
    pub(super) metadata: RwLock<Option<ProviderMetadata>>,
    identity_store: Arc<dyn identity_flight::IdentityStore>,
    identity_cooldowns: identity_cooldown::Cooldowns,
    verification_slots: tokio::sync::Semaphore,
    cookie_verifications: cookie_authorization::VerificationFlights,
    bearer_verifications: bearer_authorization::VerificationFlights,
    pub(in crate::auth) signup_options_cache: signup_options::SignupOptionsCache,
    pub(super) store: Arc<dyn AuthStore>,
    pub(super) records: Option<SignInRecords>,
    spend: Option<Arc<dyn registration::RegistrationPolicy>>,
    registration_capacity: Option<Arc<dyn registration::RegistrationCapacity>>,
}

pub struct RequestAuth {
    pub binding: Option<layer2_core::sandbox::binding::BindingPrincipal>,
    pub credential_source: layer2_core::resource_access::CredentialSource,
    pub subject: SubjectId,
    pub authorization: Authorization,
    pub profile: Profile,
    pub delegated_credential: Option<String>,
}

impl AuthState {
    /// Build auth state with separate NyxID back-channel and browser origins.
    pub fn with_store_and_nyxid_public_url(
        mode: AuthMode,
        records: Option<SignInRecords>,
        store: Arc<dyn AuthStore>,
        nyxid_public_url: Option<String>,
    ) -> Arc<Self> {
        let nyxid_public_url = match (&mode, nyxid_public_url) {
            (AuthMode::Provider { .. }, Some(url)) => url,
            (AuthMode::Provider { .. }, None) => {
                panic!("provider auth requires a configured NyxID public URL")
            }
            (AuthMode::Trusted { .. }, _) => String::new(),
        };
        let browser_origins = match &mode {
            AuthMode::Provider { public_url, .. } => {
                vec![public_url.trim_end_matches('/').to_owned()]
            }
            AuthMode::Trusted { .. } => vec![],
        };
        Arc::new(Self {
            standing: Arc::new(layer2_core::account_deletion::AvailableAccounts),
            account_deletion_enabled: false,
            privacy_export_enabled: false,
            binding_diagnostics_enabled: false,
            platform_settings: None,
            bot_father_username: None,
            bindings: None,
            agent_key_service_id: None,
            oauth_api_resource: None,
            resource_request_enabled: false,
            silent_sign_in_enabled: false,
            legal_terms: None,
            browser_session_join_enabled: false,
            service_authorization: None,
            #[cfg(test)]
            preference_clock: SystemTime::now,
            identity_proxy: None,
            proxy_replay_principal_enabled: false,
            mode,
            nyxid_public_url,
            browser_origins,
            records,
            spend: None,
            registration_capacity: None,
            store,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("the rustls HTTP client builds"),
            metadata: RwLock::new(None),
            identity_store: Arc::new(identity_flight::MemoryIdentityStore::default()),
            identity_cooldowns: identity_cooldown::Cooldowns::default(),
            verification_slots: tokio::sync::Semaphore::new(64),
            cookie_verifications: cookie_authorization::VerificationFlights::default(),
            bearer_verifications: bearer_authorization::VerificationFlights::default(),
            signup_options_cache: signup_options::SignupOptionsCache::default(),
            creation_outcomes_enabled: false,
        })
    }

    pub fn with_silent_sign_in(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("auth configured before sharing")
            .silent_sign_in_enabled = enabled;
        self
    }

    pub fn requires_credential(&self) -> bool {
        matches!(self.mode, AuthMode::Provider { .. })
    }

    /// Resolve and, when possible, refresh the owner's bearer for background
    /// resource credential operations. The value never enters a resource aggregate.
    pub async fn delegated_credential_for(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<persistence::SecretValue>, StoreError> {
        let Some(mut state) = self.store.subject(subject).await? else {
            return Ok(None);
        };
        if &state.subject != subject {
            return Err(StoreError::Malformed(
                "subject credential crossed its identity boundary".into(),
            ));
        }
        refresh::refresh_if_needed(self, &mut state).await?;
        Ok(state.tokens.and_then(|tokens| {
            (tokens.expires_at > SystemTime::now()).then_some(tokens.access_token)
        }))
    }

    pub(super) fn secure_cookies(&self) -> bool {
        matches!(
            &self.mode,
            AuthMode::Provider { public_url, .. }
                if reqwest::Url::parse(public_url).is_ok_and(|url| url.scheme() == "https")
        )
    }

    pub fn trusted_authorization() -> Authorization {
        Authorization::new([
            Capability::CreateSandbox,
            Capability::UseOwnSandboxes,
            Capability::DeleteOwnSandbox,
            Capability::ManageOwnProfiles,
            Capability::PublishTriggers,
            Capability::LaunchTriggers,
        ])
    }

    pub async fn resolve_request(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<RequestAuth>, StoreError> {
        self.resolve_request_with_proxy_inventory(headers, false)
            .await
    }

    async fn resolve_request_with_proxy_inventory(
        &self,
        headers: &HeaderMap,
        personal_inventory: bool,
    ) -> Result<Option<RequestAuth>, StoreError> {
        if headers.contains_key(nyxid_proxy::HEADER) {
            if self.identity_proxy.is_some() {
                return self.proxy_request(headers, personal_inventory).await;
            }
            nyxid_proxy::note_disabled();
        }
        if let Some(token) = bearer::bearer_token(headers) {
            if agent_keys::is_agent_key(&token) {
                if self.bindings.is_some() {
                    if let Some(request) = self.binding_request(token.clone()).await? {
                        return Ok(Some(request));
                    }
                    if self.is_known_binding_key(&token).await? {
                        return Ok(None);
                    }
                }
                if self.agent_key_service_id.is_none() {
                    return Ok(None);
                }
                return identity::request_for_bearer(self, token).await;
            }
        }
        match &self.mode {
            AuthMode::Trusted { subject } => Ok(Some(RequestAuth {
                binding: None,
                credential_source: layer2_core::resource_access::CredentialSource::Retained,
                subject: SubjectId::new(subject),
                authorization: Self::trusted_authorization(),
                profile: Profile::default(),
                delegated_credential: None,
            })),
            AuthMode::Provider { .. } => {
                if let Some(cookie) = session_routes::request_cookie(headers, AUTH_COOKIE) {
                    let id = SecretDigest::identify(AUTH_COOKIE_DOMAIN, cookie);
                    if let Some((login, state)) = self.read_browser_session(&id).await? {
                        if login.expires_at <= SystemTime::now() {
                            let _ = self.store.delete_browser_login(&id).await;
                            return Ok(None);
                        }
                        let state = if self.browser_session_join_enabled {
                            state
                        } else {
                            self.store.subject(&login.subject).await?
                        };
                        let Some(mut state) = state else {
                            return Ok(Some(RequestAuth {
                                binding: None,
                                credential_source:
                                    layer2_core::resource_access::CredentialSource::Retained,
                                subject: login.subject,
                                authorization: Authorization::none(),
                                profile: Profile::default(),
                                delegated_credential: None,
                            }));
                        };
                        if state.subject != login.subject {
                            return Err(StoreError::Malformed(
                                "browser authority crossed its identity boundary".into(),
                            ));
                        }
                        let had_tokens = state.tokens.is_some();
                        refresh::refresh_if_needed(self, &mut state).await?;
                        if had_tokens && state.tokens.is_none() {
                            return Ok(None);
                        }
                        let resolved = cookie_authorization::request_for_browser_cookie(
                            self,
                            &login.subject,
                            state,
                        )
                        .await?;
                        // Every browser verification rechecks logout after IO,
                        // including logout committed by another replica.
                        if !self.store.browser_login(&id).await?.is_some_and(|current| {
                            current.subject == login.subject
                                && current.expires_at > SystemTime::now()
                        }) {
                            return Ok(None);
                        }
                        return Ok(resolved);
                    }
                }
                let Some(token) = bearer::bearer_token(headers) else {
                    return Ok(None);
                };
                identity::request_for_bearer(self, token).await
            }
        }
    }

    pub async fn store_sign_in(
        &self,
        subject: SubjectId,
        tokens: DurableSubjectTokens,
        authorization: Authorization,
        profile: Profile,
    ) -> Result<(), StoreError> {
        self.identity_store.clear();
        for _ in 0..32 {
            let mut next = SubjectAuthState::new(
                subject.clone(),
                authorization.clone(),
                profile.clone(),
                Some(tokens.clone()),
            );
            let current = match self.store.subject(&subject).await {
                Ok(current) => current,
                Err(error) => {
                    if self.store.replace_unfenced_subject(next).await? {
                        return Ok(());
                    }
                    return Err(error);
                }
            };
            let stored = if let Some(current) = current {
                next.revision = persistence::next_auth_counter(current.revision)?;
                next.token_generation = persistence::next_auth_counter(current.token_generation)?;
                self.store
                    .replace_subject(&current.incarnation, current.revision, next)
                    .await?
            } else {
                self.store.insert_subject(next).await?
            };
            if stored {
                return Ok(());
            }
        }
        Err(StoreError::Unavailable(
            "subject sign-in remained contended".into(),
        ))
    }
}

fn rebase(endpoint: &str, base: &str) -> Result<String, String> {
    let endpoint = reqwest::Url::parse(endpoint)
        .map_err(|error| format!("identity endpoint is not a URL: {error}"))?;
    let mut base = reqwest::Url::parse(base)
        .map_err(|error| format!("identity base URL is not usable: {error}"))?;
    base.set_path(endpoint.path());
    base.set_query(endpoint.query());
    base.set_fragment(None);
    Ok(base.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn trusted_mode_attaches_only_daily_authority() {
        let auth = AuthState::in_memory(
            AuthMode::Trusted {
                subject: "developer".into(),
            },
            None,
        );
        let resolved = auth
            .resolve_request(&HeaderMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.subject.as_str(), "developer");
        assert!(resolved.authorization.allows(Capability::UseOwnSandboxes));
        assert!(!resolved.authorization.allows(Capability::ConfigurePlatform));
    }

    #[test]
    fn provider_diagnostics_redact_the_client_secret() {
        let mode = AuthMode::Provider {
            base_url: "https://identity.example".into(),
            client_id: "client".into(),
            client_secret: "never-print-me".into(),
            public_url: "https://cma.example".into(),
        };
        assert!(!format!("{mode:?}").contains("never-print-me"));
    }

    #[test]
    fn endpoint_rebasing_preserves_only_the_provider_path_and_query() {
        assert_eq!(
            rebase(
                "https://public.example/oauth/token?audience=cma",
                "http://identity.internal:9090"
            )
            .unwrap(),
            "http://identity.internal:9090/oauth/token?audience=cma"
        );
    }
}
