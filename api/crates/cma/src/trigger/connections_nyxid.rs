//! NyxID service-connection adapter for Trigger Launch requirements.

use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::trigger_domain::{
    BrokerHandoff, BrokerHandoffState, ServiceConnectionBroker, ServiceConnectionError,
    ServiceRequirement, ServiceRequirementState,
};
use layer2_core::{seconds_from_rfc3339, ServiceId, StoreError, SubjectId};
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::auth::{persistence::SecretValue, AuthState};

const CONNECT_LINKS_PATH: &str = "/api/v1/connect-links";
const USER_SERVICES_PATH: &str = "/api/v1/user-services";
const CONNECT_LINK_TTL_SECONDS: u64 = 15 * 60;
const PROVIDER_UNAVAILABLE: &str = "NyxID request failed";
const PROVIDER_MALFORMED: &str = "NyxID response was not usable";
const CREDENTIAL_UNAVAILABLE: &str = "visitor credential lookup failed";

#[async_trait]
pub(crate) trait VisitorCredentialSource: Send + Sync {
    async fn credential_for(&self, owner: &SubjectId) -> Result<Option<SecretValue>, StoreError>;
}

#[async_trait]
impl VisitorCredentialSource for AuthState {
    async fn credential_for(&self, owner: &SubjectId) -> Result<Option<SecretValue>, StoreError> {
        self.delegated_credential_for(owner).await
    }
}

pub(crate) struct NyxidServiceConnectionBroker {
    http: Client,
    base_url: String,
    credentials: Arc<dyn VisitorCredentialSource>,
}

impl NyxidServiceConnectionBroker {
    pub(crate) fn new<T>(http: Client, base_url: impl Into<String>, credentials: Arc<T>) -> Self
    where
        T: VisitorCredentialSource + 'static,
    {
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            credentials,
        }
    }

    async fn credential(&self, owner: &SubjectId) -> Result<SecretValue, ServiceConnectionError> {
        self.credentials
            .credential_for(owner)
            .await
            .map_err(|_| unavailable(CREDENTIAL_UNAVAILABLE))?
            .ok_or(ServiceConnectionError::Refused)
    }

    async fn user_services(
        &self,
        credential: &SecretValue,
    ) -> Result<Vec<UserServiceRow>, ServiceConnectionError> {
        let response = self
            .send(
                self.http
                    .get(self.endpoint(USER_SERVICES_PATH))
                    .bearer_auth(credential.expose()),
                false,
            )
            .await?;
        let listing = decode(response).await?;
        Ok(UserServiceListing::into_rows(listing))
    }

    async fn send(
        &self,
        request: RequestBuilder,
        missing_is_not_found: bool,
    ) -> Result<Response, ServiceConnectionError> {
        let response = request
            .send()
            .await
            .map_err(|_| unavailable(PROVIDER_UNAVAILABLE))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        if missing_is_not_found && status == StatusCode::NOT_FOUND {
            return Err(ServiceConnectionError::NotFound);
        }
        if status.is_server_error() || matches!(status.as_u16(), 408 | 425 | 429) {
            Err(unavailable(PROVIDER_UNAVAILABLE))
        } else {
            Err(ServiceConnectionError::Refused)
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn handoff_endpoint(&self, external_id: &str) -> Result<Url, ServiceConnectionError> {
        if !valid_provider_id(external_id) {
            return Err(malformed());
        }
        let mut endpoint = Url::parse(&format!("{}{CONNECT_LINKS_PATH}/", self.base_url))
            .map_err(|_| malformed())?;
        endpoint
            .path_segments_mut()
            .map_err(|_| malformed())?
            .pop_if_empty()
            .push(external_id);
        Ok(endpoint)
    }

    async fn completed_state(
        &self,
        credential: &SecretValue,
        connected: Option<ConnectedService>,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        let Some(connected) = connected.filter(ConnectedService::is_valid) else {
            return Ok(BrokerHandoffState::Failed);
        };
        let rows = self.user_services(credential).await?;
        let mut matches = rows.iter().filter(|row| row.id == connected.id);
        let active = matches
            .next()
            .filter(|row| row.slug == connected.slug)
            .is_some_and(|row| row.is_active == Some(true));
        if active && matches.next().is_none() {
            Ok(BrokerHandoffState::Satisfied)
        } else {
            Ok(BrokerHandoffState::Failed)
        }
    }
}

#[async_trait]
impl ServiceConnectionBroker for NyxidServiceConnectionBroker {
    async fn requirements(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        if services.is_empty() {
            return Ok(Vec::new());
        }
        let credential = self.credential(owner).await?;
        let rows = self.user_services(&credential).await?;
        Ok(services
            .iter()
            .map(|service| ServiceRequirement {
                service: service.clone(),
                state: requirement_state(service, &rows),
            })
            .collect())
    }

    async fn create_handoff(
        &self,
        owner: &SubjectId,
        service: &ServiceId,
        _intent_id: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        if service.as_str().trim().is_empty() {
            return Err(ServiceConnectionError::Refused);
        }
        let credential = self.credential(owner).await?;
        let response = self
            .send(
                self.http
                    .post(self.endpoint(CONNECT_LINKS_PATH))
                    .bearer_auth(credential.expose())
                    .json(&CreateConnectLinkRequest {
                        service_slug: service.as_str(),
                        expires_in: CONNECT_LINK_TTL_SECONDS,
                    }),
                false,
            )
            .await?;
        created_handoff(decode(response).await?)
    }

    async fn handoff_state(
        &self,
        owner: &SubjectId,
        external_id: &str,
        service: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        let credential = self.credential(owner).await?;
        let endpoint = self.handoff_endpoint(external_id)?;
        let response = self
            .send(
                self.http.get(endpoint).bearer_auth(credential.expose()),
                true,
            )
            .await?;
        let link: ConnectLinkResponse = decode(response).await?;
        if link.id != external_id
            || link.service_slug != service.as_str()
            || seconds_from_rfc3339(&link.expires_at).is_none()
        {
            return Err(malformed());
        }
        match link.status {
            ConnectLinkStatus::Pending => Ok(BrokerHandoffState::Pending),
            ConnectLinkStatus::Expired => Ok(BrokerHandoffState::Expired),
            ConnectLinkStatus::Cancelled => Ok(BrokerHandoffState::Cancelled),
            ConnectLinkStatus::Completed => {
                self.completed_state(&credential, link.connected_service)
                    .await
            }
        }
    }
}

#[derive(Serialize)]
struct CreateConnectLinkRequest<'a> {
    service_slug: &'a str,
    expires_in: u64,
}

#[derive(Deserialize)]
struct CreateConnectLinkResponse {
    id: String,
    connect_url: String,
    expires_at: String,
}

#[derive(Deserialize)]
struct ConnectLinkResponse {
    id: String,
    status: ConnectLinkStatus,
    service_slug: String,
    expires_at: String,
    #[serde(default)]
    connected_service: Option<ConnectedService>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ConnectLinkStatus {
    Pending,
    Completed,
    Expired,
    Cancelled,
}

#[derive(Deserialize)]
struct ConnectedService {
    id: String,
    slug: String,
}

impl ConnectedService {
    fn is_valid(&self) -> bool {
        valid_provider_id(&self.id) && !self.slug.trim().is_empty()
    }
}

#[derive(Deserialize)]
struct UserServiceRow {
    id: String,
    slug: String,
    #[serde(default)]
    is_active: Option<bool>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum UserServiceListing {
    Bare(Vec<UserServiceRow>),
    Wrapped {
        #[serde(alias = "items", alias = "user_services")]
        services: Vec<UserServiceRow>,
    },
}

impl UserServiceListing {
    fn into_rows(self) -> Vec<UserServiceRow> {
        match self {
            Self::Bare(rows) => rows,
            Self::Wrapped { services } => services,
        }
    }
}

async fn decode<T: DeserializeOwned>(response: Response) -> Result<T, ServiceConnectionError> {
    response.json().await.map_err(|_| malformed())
}

fn created_handoff(
    response: CreateConnectLinkResponse,
) -> Result<BrokerHandoff, ServiceConnectionError> {
    let hosted_url = Url::parse(&response.connect_url).map_err(|_| malformed())?;
    if !valid_provider_id(&response.id)
        || !matches!(hosted_url.scheme(), "http" | "https")
        || hosted_url.host_str().is_none()
        || !hosted_url.username().is_empty()
        || hosted_url.password().is_some()
        || seconds_from_rfc3339(&response.expires_at).is_none()
    {
        return Err(malformed());
    }
    Ok(BrokerHandoff {
        external_id: response.id,
        hosted_url: response.connect_url,
        expires_at: response.expires_at,
    })
}

fn requirement_state(service: &ServiceId, rows: &[UserServiceRow]) -> ServiceRequirementState {
    let exact = rows
        .iter()
        .filter(|row| row.id == service.as_str())
        .collect::<Vec<_>>();
    let matches = if exact.is_empty() {
        rows.iter()
            .filter(|row| row.slug == service.as_str())
            .collect::<Vec<_>>()
    } else {
        exact
    };
    match matches.as_slice() {
        [] => ServiceRequirementState::Missing,
        [row] => match row.is_active {
            Some(true) => ServiceRequirementState::Satisfied,
            Some(false) => ServiceRequirementState::Inactive,
            None => ServiceRequirementState::Unknown,
        },
        _ => ServiceRequirementState::Unknown,
    }
}

fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn unavailable(detail: &str) -> ServiceConnectionError {
    ServiceConnectionError::Unavailable(detail.to_owned())
}

fn malformed() -> ServiceConnectionError {
    ServiceConnectionError::Malformed(PROVIDER_MALFORMED.to_owned())
}
