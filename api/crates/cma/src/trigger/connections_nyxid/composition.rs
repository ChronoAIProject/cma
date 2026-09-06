use std::{sync::Arc, time::Duration};

use layer2_core::trigger_domain::{
    BrokerHandoff, BrokerHandoffState, ServiceConnectionBroker, ServiceConnectionError,
    ServiceConnectionMetadata, ServiceRequirement,
};
use layer2_core::{ServiceId, SubjectId};

use super::NyxidServiceConnectionBroker;
use crate::auth::{AuthMode, AuthState};

const PROVIDER_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const INVALID_PROVIDER_CONFIGURATION: &str =
    "NyxID service connection configuration was not usable";

/// The service-connection boundary available in every supported auth mode.
pub(crate) struct CmaServiceConnectionBroker {
    mode: BrokerMode,
}

enum BrokerMode {
    Provider(NyxidServiceConnectionBroker),
    Trusted,
}

impl CmaServiceConnectionBroker {
    /// Build the broker from the same auth configuration used by request auth.
    pub(crate) fn from_auth_config(
        auth_mode: &AuthMode,
        nyxid_public_url: Option<&str>,
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
                NyxidServiceConnectionBroker::new(http, base_url, browser_origin, auth).map(
                    |broker| Self {
                        mode: BrokerMode::Provider(broker),
                    },
                )
            }
            AuthMode::Trusted { .. } => Ok(Self {
                mode: BrokerMode::Trusted,
            }),
        }
    }
}

#[async_trait::async_trait]
impl ServiceConnectionBroker for CmaServiceConnectionBroker {
    async fn describe_services(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceConnectionMetadata>, ServiceConnectionError> {
        match &self.mode {
            BrokerMode::Provider(broker) => broker.describe_services(owner, services).await,
            BrokerMode::Trusted => Ok(Vec::new()),
        }
    }

    async fn requirements(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        match &self.mode {
            BrokerMode::Provider(broker) => broker.requirements(owner, services).await,
            BrokerMode::Trusted if services.is_empty() => Ok(Vec::new()),
            BrokerMode::Trusted => Err(ServiceConnectionError::Refused),
        }
    }

    async fn create_handoff(
        &self,
        owner: &SubjectId,
        service: &ServiceId,
        intent_id: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
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
        match &self.mode {
            BrokerMode::Provider(broker) => broker.handoff_state(owner, external_id, service).await,
            BrokerMode::Trusted => Err(ServiceConnectionError::Refused),
        }
    }
}

fn invalid_provider_configuration() -> ServiceConnectionError {
    ServiceConnectionError::Malformed(INVALID_PROVIDER_CONFIGURATION.to_owned())
}

#[cfg(test)]
#[path = "composition_tests.rs"]
mod tests;
