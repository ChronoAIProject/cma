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
    pub(super) catalog_service_id: Option<String>,
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

fn service_row_rank(row: &UserServiceRow) -> u8 {
    let active = row.is_active == Some(true);
    match (&row.credential_source, active, row.is_allowed()) {
        (Some(UserServiceCredentialSource::Personal), true, true) => 0,
        (Some(UserServiceCredentialSource::Org { allowed: true }), true, true) => 1,
        (None, true, true) => 2,
        (Some(UserServiceCredentialSource::Personal), false, true) => 3,
        (Some(UserServiceCredentialSource::Org { allowed: true }), false, true) => 4,
        (None, false, true) => 5,
        // A denied organization row is retained as the last fallback so a
        // single denied binding remains Unknown and is never probed.
        (Some(UserServiceCredentialSource::Org { allowed: false }), _, _) => 6,
        // Keep the selector total if the policy predicate grows new source
        // variants in the future.
        (_, _, false) => 6,
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

/// Select the user-service row that represents a slug request.
///
/// NyxID can legitimately expose both a personal and an organization row for
/// the same slug. Exact IDs remain strict: duplicate exact bindings are
/// rejected by returning `None`. Slug lookups use the same deterministic
/// preference everywhere in CMA so requirement checks, catalog metadata, and
/// credential probes cannot disagree about which binding is active.
pub(super) fn selected_service<'a>(
    service: &ServiceId,
    rows: &'a [UserServiceRow],
) -> Option<&'a UserServiceRow> {
    let exact = rows
        .iter()
        .filter(|row| row.id == service.as_str())
        .collect::<Vec<_>>();
    if !exact.is_empty() {
        return (exact.len() == 1).then_some(exact[0]);
    }

    rows.iter()
        .filter(|row| row.slug == service.as_str())
        .min_by_key(|row| (service_row_rank(row), row.id.as_str()))
}

pub(super) fn requirement_state(
    service: &ServiceId,
    rows: &[UserServiceRow],
) -> ServiceRequirementState {
    let matches = matching_services(service, rows);
    let Some(row) = selected_service(service, rows) else {
        return if matches.is_empty() {
            ServiceRequirementState::Missing
        } else {
            ServiceRequirementState::Unknown
        };
    };
    if !row.is_allowed() {
        return ServiceRequirementState::Unknown;
    }
    match row.is_active {
        Some(true) => ServiceRequirementState::Satisfied,
        Some(false) => ServiceRequirementState::Inactive,
        None => ServiceRequirementState::Unknown,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        id: &str,
        slug: &str,
        is_active: Option<bool>,
        credential_source: Option<UserServiceCredentialSource>,
    ) -> UserServiceRow {
        UserServiceRow {
            id: id.to_owned(),
            slug: slug.to_owned(),
            catalog_service_id: None,
            is_active,
            credential_source,
        }
    }

    #[test]
    fn slug_prefers_active_personal_over_allowed_organization() {
        let rows = vec![
            row(
                "org-row",
                "chrono-llm-public",
                Some(true),
                Some(UserServiceCredentialSource::Org { allowed: true }),
            ),
            row(
                "personal-row",
                "chrono-llm-public",
                Some(true),
                Some(UserServiceCredentialSource::Personal),
            ),
        ];
        let service = ServiceId::new("chrono-llm-public");
        assert_eq!(
            selected_service(&service, &rows).map(|row| row.id.as_str()),
            Some("personal-row")
        );
        assert_eq!(
            requirement_state(&service, &rows),
            ServiceRequirementState::Satisfied
        );
    }

    #[test]
    fn slug_falls_back_to_allowed_organization_when_personal_is_unusable() {
        let rows = vec![
            row(
                "personal-row",
                "ornn-api",
                Some(false),
                Some(UserServiceCredentialSource::Personal),
            ),
            row(
                "org-row",
                "ornn-api",
                Some(true),
                Some(UserServiceCredentialSource::Org { allowed: true }),
            ),
        ];
        let service = ServiceId::new("ornn-api");
        assert_eq!(
            selected_service(&service, &rows).map(|row| row.id.as_str()),
            Some("org-row")
        );
        assert_eq!(
            requirement_state(&service, &rows),
            ServiceRequirementState::Satisfied
        );
    }

    #[test]
    fn denied_organization_is_never_preferred_over_personal() {
        let rows = vec![
            row(
                "denied-org",
                "ornn-api",
                Some(true),
                Some(UserServiceCredentialSource::Org { allowed: false }),
            ),
            row(
                "personal-row",
                "ornn-api",
                Some(true),
                Some(UserServiceCredentialSource::Personal),
            ),
        ];
        let service = ServiceId::new("ornn-api");
        assert_eq!(
            selected_service(&service, &rows).map(|row| row.id.as_str()),
            Some("personal-row")
        );
        assert_eq!(
            requirement_state(&service, &rows),
            ServiceRequirementState::Satisfied
        );
    }

    #[test]
    fn duplicate_exact_ids_remain_unknown() {
        let rows = vec![
            row("same-id", "first", Some(true), None),
            row("same-id", "second", Some(true), None),
        ];
        let service = ServiceId::new("same-id");
        assert!(selected_service(&service, &rows).is_none());
        assert_eq!(
            requirement_state(&service, &rows),
            ServiceRequirementState::Unknown
        );
    }

    #[test]
    fn same_rank_slug_rows_are_selected_by_id_for_stability() {
        let rows = vec![
            row(
                "z-org",
                "service",
                Some(true),
                Some(UserServiceCredentialSource::Org { allowed: true }),
            ),
            row(
                "a-org",
                "service",
                Some(true),
                Some(UserServiceCredentialSource::Org { allowed: true }),
            ),
        ];
        let service = ServiceId::new("service");
        assert_eq!(
            selected_service(&service, &rows).map(|row| row.id.as_str()),
            Some("a-org")
        );
    }
}
