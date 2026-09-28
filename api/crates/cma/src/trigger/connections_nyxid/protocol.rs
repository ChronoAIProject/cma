use layer2_core::trigger_domain::ServiceRequirementState;
use layer2_core::{rfc3339_at, seconds_from_rfc3339, ServiceId};
use reqwest::Url;
use serde::{Deserialize, Serialize};

use super::{ServiceConnectionError, PROVIDER_MALFORMED};

const CONNECT_LINK_TOKEN_PREFIX: &str = "nyx_clk_";

#[derive(Serialize)]
pub(super) struct CreateConnectLinkRequest<'a> {
    pub(super) service_slug: &'a str,
    pub(super) expires_in: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) callback_url: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CreateConnectLinkResponse {
    pub(super) id: String,
    pub(super) connect_url: String,
    pub(super) expires_at: String,
}

#[derive(Deserialize)]
pub(super) struct ConnectLinkResponse {
    pub(super) id: String,
    pub(super) status: ConnectLinkStatus,
    pub(super) service_slug: String,
    pub(super) expires_at: String,
    #[serde(default)]
    pub(super) connected_service: Option<ConnectedService>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ConnectLinkStatus {
    Pending,
    Completed,
    Expired,
    Cancelled,
}

#[derive(Deserialize)]
pub(super) struct ConnectedService {
    pub(super) id: String,
    pub(super) slug: String,
}

impl ConnectedService {
    pub(super) fn is_valid(&self) -> bool {
        valid_provider_id(&self.id) && !self.slug.trim().is_empty()
    }
}

#[derive(Deserialize)]
pub(super) struct UserServiceRow {
    pub(super) id: String,
    pub(super) slug: String,
    #[serde(default)]
    pub(super) is_active: Option<bool>,
    #[serde(default)]
    credential_source: Option<UserServiceCredentialSource>,
}

impl UserServiceRow {
    pub(super) fn is_allowed(&self) -> bool {
        !matches!(
            &self.credential_source,
            Some(UserServiceCredentialSource::Org { allowed: false })
        )
    }

    pub(super) fn is_usable(&self) -> bool {
        self.is_active == Some(true) && self.is_allowed()
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum UserServiceCredentialSource {
    Personal,
    Org { allowed: bool },
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum UserServiceListing {
    Bare(Vec<UserServiceRow>),
    Wrapped {
        #[serde(alias = "items", alias = "user_services")]
        services: Vec<UserServiceRow>,
    },
}

impl UserServiceListing {
    pub(super) fn into_rows(self) -> Vec<UserServiceRow> {
        match self {
            Self::Bare(rows) => rows,
            Self::Wrapped { services } => services,
        }
    }
}

pub(super) fn matching_services<'a>(
    service: &ServiceId,
    rows: &'a [UserServiceRow],
) -> Vec<&'a UserServiceRow> {
    let exact = rows
        .iter()
        .filter(|row| row.id == service.as_str())
        .collect::<Vec<_>>();
    if exact.is_empty() {
        rows.iter()
            .filter(|row| row.slug == service.as_str())
            .collect::<Vec<_>>()
    } else {
        exact
    }
}

pub(super) fn requirement_state(
    service: &ServiceId,
    rows: &[UserServiceRow],
) -> ServiceRequirementState {
    let matches = matching_services(service, rows);
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

pub(super) fn canonical_browser_origin(value: &str) -> Result<Url, ServiceConnectionError> {
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

pub(super) fn canonical_provider_timestamp(value: &str) -> Result<String, ServiceConnectionError> {
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

pub(super) fn valid_connect_path(url: &Url) -> bool {
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

pub(super) fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

pub(super) fn malformed() -> ServiceConnectionError {
    ServiceConnectionError::Malformed(PROVIDER_MALFORMED.to_owned())
}
