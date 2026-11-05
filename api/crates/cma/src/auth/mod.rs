//! Browser and bearer authentication against the configured identity provider.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::http::HeaderMap;
use layer2_core::{Authorization, Capability, StoreError, SubjectId};
use tokio::sync::RwLock;

pub mod actor_links;
mod agent_keys;
pub mod bearer;
mod bearer_authorization;
mod binding;
mod bots;
mod browser_origins;
mod cookie_authorization;
pub mod deferred_commands;
pub mod entitlements;
pub mod flow;
mod identity;
#[allow(dead_code, reason = "the aggregate-neutral issuer is consumed by #809")]
pub mod keys;
mod middleware;
pub(crate) mod nyxid_proxy;
pub mod organizations;
mod permissions;
pub mod persistence;
pub mod profile;
pub(crate) mod provider_http;
#[cfg(test)]
mod public_contract_tests;
mod records;
mod refresh;
mod resource_request;
mod sandbox_prerequisite;
pub(crate) mod service_portal;
mod session_routes;
mod signup_options;
#[cfg(test)]
pub(crate) mod test_resources;
mod token;
mod tokens;
mod usage_reporting;
mod verification_error;
mod worker_credentials;

pub use flow::router;
pub use middleware::attach_subject;
pub use records::SignInRecords;
pub use tokens::ProviderMetadata;

#[cfg(test)]
use persistence::MemoryAuthStore;
use persistence::{AuthStore, BrowserLogin, DurableSubjectTokens, SecretDigest, SubjectAuthState};
use profile::Profile;

pub const AUTH_COOKIE: &str = "cma_session";
const AUTH_COOKIE_DOMAIN: &str = "browser-login";
const AUTH_COOKIE_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

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
    platform_settings: Option<Arc<layer2_core::PlatformSettingsService>>,
    bot_father_username: Option<String>,
    agent_key_service_id: Option<String>,
    resource_request_enabled: bool,
    identity_proxy: Option<nyxid_proxy::jwt::Verifier>,
    bindings: Option<Arc<layer2_core::sandbox::binding::BindingAccessService>>,
    pub(super) mode: AuthMode,
    /// Browser-facing NyxID origin used for registration handoffs.
    pub(super) nyxid_public_url: String,
    browser_origins: Vec<String>,
    pub(super) http: reqwest::Client,
    pub(super) metadata: RwLock<Option<ProviderMetadata>>,
    cookie_verifications: cookie_authorization::VerificationFlights,
    bearer_verifications: bearer_authorization::VerificationFlights,
    pub(in crate::auth) signup_options_cache: signup_options::SignupOptionsCache,
    pub(super) store: Arc<dyn AuthStore>,
    pub(super) records: Option<SignInRecords>,
}

pub(super) struct RequestAuth {
    pub binding: Option<layer2_core::sandbox::binding::BindingPrincipal>,
    pub credential_source: layer2_core::resource_access::CredentialSource,
    pub subject: SubjectId,
    pub authorization: Authorization,
    pub profile: Profile,
    pub delegated_credential: Option<String>,
}

impl AuthState {
    #[cfg(test)]
    pub fn in_memory(mode: AuthMode, records: Option<SignInRecords>) -> Arc<Self> {
        Self::with_store(mode, records, Arc::new(MemoryAuthStore::default()))
    }

    #[cfg(test)]
    pub fn with_store(
        mode: AuthMode,
        records: Option<SignInRecords>,
        store: Arc<dyn AuthStore>,
    ) -> Arc<Self> {
        let nyxid_public_url = match &mode {
            AuthMode::Provider { base_url, .. } => Some(base_url.clone()),
            AuthMode::Trusted { .. } => None,
        };
        Self::with_store_and_nyxid_public_url(mode, records, store, nyxid_public_url)
    }

    /// Build auth state with separate NyxID back-channel and browser origins.
    pub(crate) fn with_store_and_nyxid_public_url(
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
            platform_settings: None,
            bot_father_username: None,
            bindings: None,
            agent_key_service_id: None,
            resource_request_enabled: false,
            identity_proxy: None,
            mode,
            nyxid_public_url,
            browser_origins,
            records,
            store,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("the rustls HTTP client builds"),
            metadata: RwLock::new(None),
            cookie_verifications: cookie_authorization::VerificationFlights::default(),
            bearer_verifications: bearer_authorization::VerificationFlights::default(),
            signup_options_cache: signup_options::SignupOptionsCache::default(),
        })
    }

    pub fn requires_credential(&self) -> bool {
        matches!(self.mode, AuthMode::Provider { .. })
    }

    /// Resolve and, when possible, refresh the owner's bearer for background
    /// resource credential operations. The value never enters a resource aggregate.
    pub(crate) async fn delegated_credential_for(
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

    pub(super) async fn resolve_request(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<RequestAuth>, StoreError> {
        if self.identity_proxy.is_some() && headers.contains_key(nyxid_proxy::HEADER) {
            return self.proxy_request(headers).await;
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
                    if let Some(login) = self.store.browser_login(&id).await? {
                        if login.expires_at <= SystemTime::now() {
                            let _ = self.store.delete_browser_login(&id).await;
                            return Ok(None);
                        }
                        let Some(mut state) = self.store.subject(&login.subject).await? else {
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
                        refresh::refresh_if_needed(self, &mut state).await?;
                        return cookie_authorization::request_for_browser_cookie(
                            self,
                            &login.subject,
                            state,
                        )
                        .await;
                    }
                }
                let Some(token) = bearer::bearer_token(headers) else {
                    return Ok(None);
                };
                identity::request_for_bearer(self, token).await
            }
        }
    }

    pub(super) async fn issue_browser_login(
        &self,
        subject: SubjectId,
    ) -> Result<String, StoreError> {
        let value = token::random_token();
        self.store
            .put_browser_login(BrowserLogin {
                id: SecretDigest::identify(AUTH_COOKIE_DOMAIN, &value),
                subject,
                expires_at: SystemTime::now() + AUTH_COOKIE_LIFETIME,
            })
            .await?;
        Ok(value)
    }

    pub(super) async fn revoke_browser_login(&self, value: &str) -> Result<(), StoreError> {
        self.store
            .delete_browser_login(&SecretDigest::identify(AUTH_COOKIE_DOMAIN, value))
            .await
    }

    pub(super) async fn store_sign_in(
        &self,
        subject: SubjectId,
        tokens: DurableSubjectTokens,
        authorization: Authorization,
        profile: Profile,
    ) -> Result<(), StoreError> {
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

    pub(super) async fn metadata(&self) -> Result<ProviderMetadata, String> {
        if let Some(metadata) = self.metadata.read().await.clone() {
            return Ok(metadata);
        }
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return Err("no identity provider is configured".into());
        };
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            base_url.trim_end_matches('/')
        );
        let response = self
            .http
            .get(&discovery_url)
            .send()
            .await
            .map_err(|error| {
                identity::verification_failure("discovery", "transport", None);
                format!("identity discovery request failed: {error}")
            })?;
        if !response.status().is_success() {
            identity::verification_failure(
                "discovery",
                "http_status",
                Some(response.status().as_u16()),
            );
            return Err(format!(
                "identity discovery returned HTTP {}",
                response.status().as_u16()
            ));
        }
        let mut metadata = response
            .json::<ProviderMetadata>()
            .await
            .map_err(|error| format!("identity discovery document is invalid: {error}"))?;
        metadata.token_endpoint = rebase(&metadata.token_endpoint, base_url)?;
        metadata.userinfo_endpoint = metadata
            .userinfo_endpoint
            .as_deref()
            .map(|endpoint| rebase(endpoint, base_url))
            .transpose()?;
        metadata.introspection_endpoint = metadata
            .introspection_endpoint
            .as_deref()
            .map(|endpoint| rebase(endpoint, base_url))
            .transpose()?;
        *self.metadata.write().await = Some(metadata.clone());
        Ok(metadata)
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
