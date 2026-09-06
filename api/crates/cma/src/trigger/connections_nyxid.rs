//! NyxID service-connection adapter for Trigger Launch requirements.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt as _;
use layer2_core::trigger_domain::{
    BrokerHandoff, BrokerHandoffState, ServiceConnectionBroker, ServiceConnectionError,
    ServiceConnectionMetadata, ServiceRequirement, ServiceRequirementState,
};
use layer2_core::{rfc3339_at, seconds_from_rfc3339, ServiceId, StoreError, SubjectId};
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::auth::{persistence::SecretValue, AuthState};

mod catalog;
mod composition;

#[allow(
    unused_imports,
    reason = "exported for the production Trigger composition introduced by #824"
)]
pub(crate) use composition::CmaServiceConnectionBroker;

const CONNECT_LINKS_PATH: &str = "/api/v1/connect-links";
const USER_SERVICES_PATH: &str = "/api/v1/user-services";
const CONNECT_LINK_TOKEN_PREFIX: &str = "nyx_clk_";
const CONNECT_LINK_TTL_SECONDS: u64 = 15 * 60;
const PROVIDER_RESPONSE_MAX_BYTES: usize = 1024 * 1024;
const USER_SERVICES_MAX_ROWS: usize = 1_000;
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
    browser_origin: Url,
    credentials: Arc<dyn VisitorCredentialSource>,
}

impl NyxidServiceConnectionBroker {
    pub(crate) fn new<T>(
        http: Client,
        base_url: impl Into<String>,
        browser_origin: impl AsRef<str>,
        credentials: Arc<T>,
    ) -> Result<Self, ServiceConnectionError>
    where
        T: VisitorCredentialSource + 'static,
    {
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            browser_origin: canonical_browser_origin(browser_origin.as_ref())?,
            credentials,
        })
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
        let rows = UserServiceListing::into_rows(listing);
        if rows.len() > USER_SERVICES_MAX_ROWS {
            return Err(malformed());
        }
        Ok(rows)
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
            .is_some_and(UserServiceRow::is_usable);
        if active && matches.next().is_none() {
            Ok(BrokerHandoffState::Satisfied)
        } else {
            Ok(BrokerHandoffState::Failed)
        }
    }

    fn created_handoff(
        &self,
        response: CreateConnectLinkResponse,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        let hosted_url = Url::parse(&response.connect_url).map_err(|_| malformed())?;
        if !valid_provider_id(&response.id)
            || hosted_url.origin() != self.browser_origin.origin()
            || !hosted_url.username().is_empty()
            || hosted_url.password().is_some()
            || hosted_url.query().is_some()
            || hosted_url.fragment().is_some()
            || !valid_connect_path(&hosted_url)
        {
            return Err(malformed());
        }
        Ok(BrokerHandoff {
            external_id: response.id,
            hosted_url: hosted_url.to_string(),
            expires_at: canonical_provider_timestamp(&response.expires_at)?,
        })
    }
}

#[async_trait]
impl ServiceConnectionBroker for NyxidServiceConnectionBroker {
    async fn describe_services(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceConnectionMetadata>, ServiceConnectionError> {
        self.catalog_metadata(owner, services).await
    }

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
        self.created_handoff(decode(response).await?)
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
        if link.id != external_id || link.service_slug != service.as_str() {
            return Err(malformed());
        }
        canonical_provider_timestamp(&link.expires_at)?;
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
    #[serde(default)]
    credential_source: Option<UserServiceCredentialSource>,
}

impl UserServiceRow {
    fn is_allowed(&self) -> bool {
        !matches!(
            &self.credential_source,
            Some(UserServiceCredentialSource::Org { allowed: false })
        )
    }

    fn is_usable(&self) -> bool {
        self.is_active == Some(true) && self.is_allowed()
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UserServiceCredentialSource {
    Personal,
    Org { allowed: bool },
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
    if response
        .content_length()
        .is_some_and(|length| length > PROVIDER_RESPONSE_MAX_BYTES as u64)
    {
        return Err(malformed());
    }
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default()
            .min(PROVIDER_RESPONSE_MAX_BYTES),
    );
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| unavailable(PROVIDER_UNAVAILABLE))?;
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > PROVIDER_RESPONSE_MAX_BYTES)
        {
            return Err(malformed());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| malformed())
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
        [row] if !row.is_allowed() => ServiceRequirementState::Unknown,
        [row] => match row.is_active {
            Some(true) => ServiceRequirementState::Satisfied,
            Some(false) => ServiceRequirementState::Inactive,
            None => ServiceRequirementState::Unknown,
        },
        _ => ServiceRequirementState::Unknown,
    }
}

fn canonical_browser_origin(value: &str) -> Result<Url, ServiceConnectionError> {
    let origin = Url::parse(value).map_err(|_| malformed())?;
    if origin.scheme() != "https"
        || origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || origin.origin().ascii_serialization() != value
    {
        return Err(malformed());
    }
    Ok(origin)
}

fn canonical_provider_timestamp(value: &str) -> Result<String, ServiceConnectionError> {
    let timestamp = mongodb::bson::DateTime::parse_rfc3339_str(value).map_err(|_| malformed())?;
    if timestamp.timestamp_millis() < 0 {
        return Err(malformed());
    }
    let canonical = rfc3339_at(timestamp.to_system_time());
    if seconds_from_rfc3339(&canonical).is_none() {
        return Err(malformed());
    }
    Ok(canonical)
}

fn valid_connect_path(url: &Url) -> bool {
    let Some(mut segments) = url.path_segments() else {
        return false;
    };
    segments.next() == Some("connect")
        && segments.next().is_some_and(valid_connect_token)
        && segments.next().is_none()
}

fn valid_connect_token(value: &str) -> bool {
    value
        .strip_prefix(CONNECT_LINK_TOKEN_PREFIX)
        .is_some_and(|secret| {
            secret.len() == 64 && secret.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
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
