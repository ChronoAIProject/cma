use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{now_rfc3339, seconds_from_rfc3339, Caller, LaunchId, ServiceHandoffId, ServiceId};

use super::launch_service_support::require_launcher;
use super::{
    BrokerHandoffState, LaunchStore, LaunchStoreError, ServiceConnectionBroker,
    ServiceConnectionError, ServiceConnectionHandoff, ServiceConnectionStore,
};

const MAX_SERVICE_ID_BYTES: usize = 128;
const MAX_INTENT_ID_BYTES: usize = 256;
const SAVE_ATTEMPTS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateServiceConnection {
    pub service_id: ServiceId,
    pub launch_id: LaunchId,
    pub intent_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceConnectionServiceError {
    #[error("forbidden")]
    Forbidden,
    #[error("service connection was not found")]
    NotFound,
    #[error("service connection request is invalid: {0}")]
    InvalidRequest(String),
    #[error("intent_id is already bound to a different service connection")]
    IntentReused,
    #[error("service connection changed while its state was saved")]
    Conflict,
    #[error("service connection provider is unavailable: {0}")]
    Unavailable(String),
    #[error("service connection data is not usable: {0}")]
    Corrupt(String),
}

impl ServiceConnectionServiceError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::NotFound => "service_connection_not_found",
            Self::InvalidRequest(_) => "invalid_service_connection",
            Self::IntentReused => "service_connection_intent_reused",
            Self::Conflict => "service_connection_changed",
            Self::Unavailable(_) => "service_connection_unavailable",
            Self::Corrupt(_) => "stored_service_connection_corrupt",
        }
    }
}

pub struct ServiceConnectionService {
    launches: Arc<dyn LaunchStore>,
    handoffs: Arc<dyn ServiceConnectionStore>,
    broker: Arc<dyn ServiceConnectionBroker>,
}

impl ServiceConnectionService {
    pub fn new<L, B>(store: Arc<L>, broker: Arc<B>) -> Arc<Self>
    where
        L: LaunchStore + ServiceConnectionStore + 'static,
        B: ServiceConnectionBroker + 'static,
    {
        let launches: Arc<dyn LaunchStore> = store.clone();
        let handoffs: Arc<dyn ServiceConnectionStore> = store;
        Arc::new(Self {
            launches,
            handoffs,
            broker,
        })
    }

    pub async fn create(
        &self,
        caller: &Caller,
        request: CreateServiceConnection,
    ) -> Result<ServiceConnectionHandoff, ServiceConnectionServiceError> {
        require_launcher(caller).map_err(|_| ServiceConnectionServiceError::Forbidden)?;
        validate_request(&request)?;
        if let Some(existing) = self
            .handoffs
            .handoff_by_intent(&caller.subject, &request.launch_id, &request.intent_id)
            .await
            .map_err(store_error)?
        {
            return replay(existing, &request.service_id);
        }
        let launch = self
            .launches
            .launch(&request.launch_id, &caller.subject)
            .await
            .map_err(store_error)?
            .filter(|launch| !launch.state.is_terminal())
            .ok_or(ServiceConnectionServiceError::NotFound)?;
        let needs_connection = launch.requirements.services.iter().any(|requirement| {
            requirement.service == request.service_id && requirement.needs_connection()
        });
        if !needs_connection {
            return Err(ServiceConnectionServiceError::InvalidRequest(
                "service_id is not an unmet Launch requirement".into(),
            ));
        }
        let broker = self
            .broker
            .create_handoff(&caller.subject, &request.service_id, &request.intent_id)
            .await
            .map_err(broker_error)?;
        let now = now_rfc3339();
        let handoff = ServiceConnectionHandoff {
            id: ServiceHandoffId::generate(),
            launch_id: request.launch_id,
            owner: caller.subject.clone(),
            service_id: request.service_id.clone(),
            intent_id: request.intent_id,
            hosted_url: broker.hosted_url,
            expires_at: broker.expires_at,
            state: BrokerHandoffState::Pending,
            created_at: now.clone(),
            updated_at: now,
            external_id: broker.external_id,
            revision: 1,
        };
        handoff
            .validate()
            .map_err(|error| ServiceConnectionServiceError::Corrupt(error.to_string()))?;
        let stored = self
            .handoffs
            .create_handoff(handoff)
            .await
            .map_err(store_error)?;
        replay(stored, &request.service_id)
    }

    pub async fn read_and_refresh(
        &self,
        caller: &Caller,
        id: &ServiceHandoffId,
    ) -> Result<ServiceConnectionHandoff, ServiceConnectionServiceError> {
        require_launcher(caller).map_err(|_| ServiceConnectionServiceError::Forbidden)?;
        let mut handoff = self.owned(id, caller).await?;
        if handoff.state != BrokerHandoffState::Pending {
            return Ok(handoff);
        }
        let now = now_rfc3339();
        let now_seconds = seconds_from_rfc3339(&now)
            .ok_or_else(|| ServiceConnectionServiceError::Corrupt("clock is invalid".into()))?;
        let expires = seconds_from_rfc3339(&handoff.expires_at).ok_or_else(|| {
            ServiceConnectionServiceError::Corrupt("handoff expiry is invalid".into())
        })?;
        let observed = if expires <= now_seconds {
            BrokerHandoffState::Expired
        } else {
            match self
                .broker
                .handoff_state(&caller.subject, &handoff.external_id, &handoff.service_id)
                .await
            {
                Ok(state) => state,
                Err(ServiceConnectionError::NotFound | ServiceConnectionError::Refused) => {
                    BrokerHandoffState::Failed
                }
                Err(error) => return Err(broker_error(error)),
            }
        };
        if observed == BrokerHandoffState::Pending {
            return Ok(handoff);
        }
        for _ in 0..SAVE_ATTEMPTS {
            if handoff.state != BrokerHandoffState::Pending {
                return Ok(handoff);
            }
            let expected = handoff.revision;
            handoff.state = observed;
            handoff.updated_at = now.clone();
            handoff.revision = expected.checked_add(1).ok_or_else(|| {
                ServiceConnectionServiceError::Corrupt("revision overflow".into())
            })?;
            match self.handoffs.save_handoff(expected, handoff.clone()).await {
                Ok(saved) => return Ok(saved),
                Err(LaunchStoreError::Conflict { .. }) => {
                    handoff = self.owned(id, caller).await?;
                }
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(ServiceConnectionServiceError::Conflict)
    }

    async fn owned(
        &self,
        id: &ServiceHandoffId,
        caller: &Caller,
    ) -> Result<ServiceConnectionHandoff, ServiceConnectionServiceError> {
        self.handoffs
            .handoff(id, &caller.subject)
            .await
            .map_err(store_error)?
            .ok_or(ServiceConnectionServiceError::NotFound)
    }
}

fn validate_request(
    request: &CreateServiceConnection,
) -> Result<(), ServiceConnectionServiceError> {
    if request.service_id.as_str().trim().is_empty()
        || request.service_id.as_str().len() > MAX_SERVICE_ID_BYTES
        || request.intent_id.trim().is_empty()
        || request.intent_id.len() > MAX_INTENT_ID_BYTES
        || !request.launch_id.as_str().starts_with("lch_")
        || request.launch_id.as_str().len() == 4
    {
        return Err(ServiceConnectionServiceError::InvalidRequest(
            "service_id, launch_id, or intent_id is invalid".into(),
        ));
    }
    Ok(())
}

fn replay(
    handoff: ServiceConnectionHandoff,
    service: &ServiceId,
) -> Result<ServiceConnectionHandoff, ServiceConnectionServiceError> {
    if &handoff.service_id == service {
        Ok(handoff)
    } else {
        Err(ServiceConnectionServiceError::IntentReused)
    }
}

fn broker_error(error: ServiceConnectionError) -> ServiceConnectionServiceError {
    match error {
        ServiceConnectionError::Malformed(detail) => ServiceConnectionServiceError::Corrupt(detail),
        ServiceConnectionError::Unavailable(detail) => {
            ServiceConnectionServiceError::Unavailable(detail)
        }
        ServiceConnectionError::NotFound | ServiceConnectionError::Refused => {
            ServiceConnectionServiceError::InvalidRequest(error.to_string())
        }
    }
}

fn store_error(error: LaunchStoreError) -> ServiceConnectionServiceError {
    match error {
        LaunchStoreError::NotFound => ServiceConnectionServiceError::NotFound,
        LaunchStoreError::CommandConflict => ServiceConnectionServiceError::IntentReused,
        LaunchStoreError::Conflict { .. } | LaunchStoreError::LeaseHeld => {
            ServiceConnectionServiceError::Conflict
        }
        LaunchStoreError::Unavailable(detail) => ServiceConnectionServiceError::Unavailable(detail),
        LaunchStoreError::Corrupt(detail) => ServiceConnectionServiceError::Corrupt(detail),
        other => ServiceConnectionServiceError::Corrupt(other.to_string()),
    }
}
