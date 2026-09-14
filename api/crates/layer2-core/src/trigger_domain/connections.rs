use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{seconds_from_rfc3339, LaunchId, ServiceHandoffId, ServiceId, SubjectId};

use super::{ServiceConnectionMetadata, ServiceRequirement, TriggerError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerHandoffState {
    Pending,
    Satisfied,
    Expired,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerHandoff {
    pub external_id: String,
    pub hosted_url: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConnectionHandoff {
    pub id: ServiceHandoffId,
    pub launch_id: LaunchId,
    pub owner: SubjectId,
    pub service_id: ServiceId,
    pub intent_id: String,
    pub hosted_url: String,
    pub expires_at: String,
    pub state: BrokerHandoffState,
    pub created_at: String,
    pub updated_at: String,
    pub external_id: String,
    pub revision: u64,
}

impl ServiceConnectionHandoff {
    pub fn validate(&self) -> Result<(), TriggerError> {
        let url = url::Url::parse(&self.hosted_url).map_err(|_| {
            TriggerError::new("invalid_service_handoff", "service_connection.hosted_url")
        })?;
        if self.intent_id.trim().is_empty()
            || self.intent_id.len() > 256
            || self.service_id.as_str().trim().is_empty()
            || self.external_id.trim().is_empty()
            || !matches!(url.scheme(), "https" | "http")
            || seconds_from_rfc3339(&self.expires_at).is_none()
            || seconds_from_rfc3339(&self.created_at).is_none()
            || seconds_from_rfc3339(&self.updated_at).is_none()
            || self.expires_at <= self.created_at
            || self.revision == 0
        {
            return Err(TriggerError::new(
                "invalid_service_handoff",
                "service_connection",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceConnectionError {
    #[error("service connection was not found")]
    NotFound,
    #[error("service connection was refused")]
    Refused,
    #[error("service connection provider is unavailable: {0}")]
    Unavailable(String),
    #[error("service connection provider returned malformed data: {0}")]
    Malformed(String),
}

#[async_trait]
pub trait ServiceConnectionBroker: Send + Sync {
    /// Optional current catalog guidance. Absence must never imply hosted support.
    async fn describe_services(
        &self,
        _owner: &SubjectId,
        _services: &[ServiceId],
    ) -> Result<Vec<ServiceConnectionMetadata>, ServiceConnectionError> {
        Ok(Vec::new())
    }

    async fn requirements(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError>;

    async fn create_handoff(
        &self,
        owner: &SubjectId,
        service: &ServiceId,
        intent_id: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError>;

    async fn handoff_state(
        &self,
        owner: &SubjectId,
        external_id: &str,
        service: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError>;
}
