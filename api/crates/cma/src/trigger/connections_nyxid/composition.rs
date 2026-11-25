use std::sync::Arc;

use layer2_core::trigger_domain::{
    BrokerHandoff, BrokerHandoffState, ServiceConnectionBroker, ServiceConnectionError,
    ServiceConnectionMetadata, ServiceConnectionMode, ServiceRequirement, ServiceRequirementState,
};
use layer2_core::{ServiceId, SubjectId};

use super::{NyxidServiceConnectionBroker, VerifiedGitHubConnection, PROVIDER_HTTP_TIMEOUT};
use crate::auth::{AuthMode, AuthState};
use crate::connections::{CodexSubscriptions, SubscriptionError};
use layer3_adapter::codex::CODEX_SUBSCRIPTION_CONNECTION;

const INVALID_PROVIDER_CONFIGURATION: &str =
    "NyxID service connection configuration was not usable";

/// The service-connection boundary available in every supported auth mode.
pub(crate) struct CmaServiceConnectionBroker {
    mode: BrokerMode,
    subscriptions: Option<Arc<CodexSubscriptions>>,
}

enum BrokerMode {
    Provider(Box<NyxidServiceConnectionBroker>),
    Trusted,
}

impl CmaServiceConnectionBroker {
    #[cfg(test)]
    pub(in crate::trigger) fn with_fixture_provider(broker: NyxidServiceConnectionBroker) -> Self {
        Self {
            mode: BrokerMode::Provider(Box::new(broker)),
            subscriptions: None,
        }
    }

    pub(crate) async fn github_connections(
        &self,
        owner: &SubjectId,
        observe_accounts: bool,
    ) -> Option<super::GitHubConnections> {
        match &self.mode {
            BrokerMode::Provider(broker) => {
                Some(broker.github_connections(owner, observe_accounts).await)
            }
            BrokerMode::Trusted => None,
        }
    }

    pub(crate) async fn verify_github_repository(
        &self,
        owner: &SubjectId,
        repository: &layer1_public::rest::AgentLifecycleRepository,
        retained_connection: Option<&str>,
        pinned_commit: Option<&str>,
    ) -> super::GitHubRepositoryCheck {
        match &self.mode {
            BrokerMode::Provider(broker) => {
                broker
                    .verify_github_repository(owner, repository, retained_connection, pinned_commit)
                    .await
            }
            BrokerMode::Trusted => super::GitHubRepositoryCheck::Scope(
                super::GitHubScopeCheck::VerificationUnavailable,
            ),
        }
    }

    pub(crate) async fn exact_github_scope_check(
        &self,
        owner: &SubjectId,
        connection_id: &str,
        need: super::GitHubRepositoryNeed,
    ) -> super::GitHubScopeCheck {
        match &self.mode {
            BrokerMode::Provider(broker) => {
                broker
                    .exact_github_scope_check(owner, connection_id, need)
                    .await
            }
            BrokerMode::Trusted => super::GitHubScopeCheck::VerificationUnavailable,
        }
    }

    pub(crate) async fn verify_github_repo_token(
        &self,
        credential: &crate::auth::persistence::SecretValue,
        connection_id: &str,
    ) -> Result<VerifiedGitHubConnection, ServiceConnectionError> {
        match &self.mode {
            BrokerMode::Provider(broker) => {
                broker
                    .verify_github_repo_token(
                        credential,
                        connection_id,
                        tokio::time::Instant::now() + PROVIDER_HTTP_TIMEOUT,
                    )
                    .await
            }
            BrokerMode::Trusted => Err(ServiceConnectionError::Refused),
        }
    }

    pub(crate) async fn verify_github_token_for(
        &self,
        credential: &crate::auth::persistence::SecretValue,
        connection_id: &str,
        need: super::GitHubRepositoryNeed,
    ) -> Result<VerifiedGitHubConnection, ServiceConnectionError> {
        match &self.mode {
            BrokerMode::Provider(broker) => {
                broker
                    .verify_github_token_for(
                        credential,
                        connection_id,
                        Some(need),
                        tokio::time::Instant::now() + PROVIDER_HTTP_TIMEOUT,
                    )
                    .await
            }
            BrokerMode::Trusted => Err(ServiceConnectionError::Refused),
        }
    }

    /// Build the broker from the same auth configuration used by request auth.
    #[allow(
        dead_code,
        reason = "callback-free constructor remains useful for isolated broker tests"
    )]
    pub(crate) fn from_auth_config(
        auth_mode: &AuthMode,
        nyxid_public_url: Option<&str>,
        auth: Arc<AuthState>,
    ) -> Result<Self, ServiceConnectionError> {
        Self::from_auth_config_with_callback(auth_mode, nyxid_public_url, None, auth)
    }

    pub(crate) fn from_auth_config_with_callback(
        auth_mode: &AuthMode,
        nyxid_public_url: Option<&str>,
        cma_public_url: Option<&str>,
        auth: Arc<AuthState>,
    ) -> Result<Self, ServiceConnectionError> {
        match auth_mode {
            AuthMode::Provider { base_url, .. } => {
                let browser_origin = nyxid_public_url.ok_or_else(invalid_provider_configuration)?;
                let http = reqwest::Client::builder()
                    .timeout(PROVIDER_HTTP_TIMEOUT)
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .expect("the rustls HTTP client builds");
                NyxidServiceConnectionBroker::new_with_callback(
                    http,
                    base_url,
                    browser_origin,
                    cma_public_url,
                    auth,
                )
                .map(|broker| Self {
                    mode: BrokerMode::Provider(Box::new(broker)),
                    subscriptions: None,
                })
            }
            AuthMode::Trusted { .. } => Ok(Self {
                mode: BrokerMode::Trusted,
                subscriptions: None,
            }),
        }
    }

    pub(crate) fn with_subscriptions(mut self, subscriptions: Arc<CodexSubscriptions>) -> Self {
        self.subscriptions = Some(subscriptions);
        self
    }

    fn subscriptions(&self) -> Result<&Arc<CodexSubscriptions>, ServiceConnectionError> {
        self.subscriptions
            .as_ref()
            .ok_or(ServiceConnectionError::Refused)
    }
}

#[async_trait::async_trait]
impl ServiceConnectionBroker for CmaServiceConnectionBroker {
    async fn describe_services(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceConnectionMetadata>, ServiceConnectionError> {
        let integrations = nyxid_services(services);
        let mut metadata = match &self.mode {
            BrokerMode::Provider(broker) => broker.describe_services(owner, &integrations).await?,
            BrokerMode::Trusted => Vec::new(),
        };
        if services.iter().any(is_subscription) {
            metadata.push(ServiceConnectionMetadata {
                service: ServiceId::new(CODEX_SUBSCRIPTION_CONNECTION),
                display_name: Some("My Codex subscription".into()),
                connection_mode: ServiceConnectionMode::Hosted,
                setup_url: None,
            });
        }
        Ok(metadata)
    }

    async fn requirements(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        let integrations = nyxid_services(services);
        let mut requirements = match &self.mode {
            BrokerMode::Provider(broker) => broker.requirements(owner, &integrations).await?,
            BrokerMode::Trusted if integrations.is_empty() => Vec::new(),
            BrokerMode::Trusted => return Err(ServiceConnectionError::Refused),
        };
        if services.iter().any(is_subscription) {
            let state = match self.subscriptions()?.tokens(owner, false, None).await {
                Ok(_) => ServiceRequirementState::Satisfied,
                Err(SubscriptionError::SignInRequired) => ServiceRequirementState::Missing,
                Err(SubscriptionError::InvalidAccount | SubscriptionError::Expired) => {
                    ServiceRequirementState::Inactive
                }
                Err(_) => ServiceRequirementState::Unknown,
            };
            requirements.push(ServiceRequirement {
                service: ServiceId::new(CODEX_SUBSCRIPTION_CONNECTION),
                state,
            });
        }
        requirements.sort_by_key(|requirement| {
            services
                .iter()
                .position(|service| *service == requirement.service)
        });
        Ok(requirements)
    }

    async fn create_handoff(
        &self,
        owner: &SubjectId,
        service: &ServiceId,
        intent_id: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        if is_subscription(service) {
            let login = self
                .subscriptions()?
                .start(owner, intent_id)
                .await
                .map_err(subscription_error)?;
            return Ok(BrokerHandoff {
                external_id: login.intent,
                hosted_url: login.device.verification_url,
                expires_at: layer2_core::rfc3339_at(login.expires_at),
            });
        }
        match &self.mode {
            BrokerMode::Provider(broker) => broker.create_handoff(owner, service, intent_id).await,
            BrokerMode::Trusted => Err(ServiceConnectionError::Refused),
        }
    }

    async fn handoff_state(
        &self,
        owner: &SubjectId,
        external_id: &str,
        service: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        if is_subscription(service) {
            return match self.subscriptions()?.status(owner, external_id).await {
                Ok(login) => Ok(login.state),
                Err(SubscriptionError::Changed) => Ok(BrokerHandoffState::Cancelled),
                Err(error) => Err(subscription_error(error)),
            };
        }
        match &self.mode {
            BrokerMode::Provider(broker) => broker.handoff_state(owner, external_id, service).await,
            BrokerMode::Trusted => Err(ServiceConnectionError::Refused),
        }
    }
}

fn is_subscription(service: &ServiceId) -> bool {
    service.as_str() == CODEX_SUBSCRIPTION_CONNECTION
}

fn nyxid_services(services: &[ServiceId]) -> Vec<ServiceId> {
    services
        .iter()
        .filter(|service| !is_subscription(service))
        .cloned()
        .collect()
}

fn subscription_error(error: SubscriptionError) -> ServiceConnectionError {
    match error {
        SubscriptionError::Unavailable => ServiceConnectionError::Unavailable(error.to_string()),
        _ => ServiceConnectionError::Refused,
    }
}

fn invalid_provider_configuration() -> ServiceConnectionError {
    ServiceConnectionError::Malformed(INVALID_PROVIDER_CONFIGURATION.to_owned())
}

#[cfg(test)]
#[path = "composition_tests.rs"]
mod tests;
