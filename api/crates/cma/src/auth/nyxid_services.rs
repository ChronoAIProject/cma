use layer2_core::ServiceId;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct UserServiceRow {
    pub(crate) id: String,
    pub(crate) slug: String,
    #[serde(default)]
    pub(crate) catalog_service_id: Option<String>,
    #[serde(default)]
    pub(crate) is_active: Option<bool>,
    #[serde(default)]
    pub(crate) credential_source: Option<UserServiceCredentialSource>,
}

impl UserServiceRow {
    pub(crate) fn is_allowed(&self) -> bool {
        !matches!(
            &self.credential_source,
            Some(UserServiceCredentialSource::Org { allowed: false })
        )
    }

    pub(crate) fn is_usable(&self) -> bool {
        self.is_active == Some(true) && self.is_allowed()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum UserServiceCredentialSource {
    Personal,
    Org { allowed: bool },
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum UserServiceListing {
    Bare(Vec<UserServiceRow>),
    Wrapped {
        #[serde(alias = "items", alias = "user_services")]
        services: Vec<UserServiceRow>,
    },
}

impl UserServiceListing {
    pub(crate) fn into_rows(self) -> Vec<UserServiceRow> {
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

pub(crate) fn matching_services<'a>(
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
/// preference for requirement checks, catalog metadata, credential probes,
/// and key issuance. A slug may not
/// select an ID duplicated elsewhere in the listing.
pub(crate) fn selected_service<'a>(
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

    let selected = rows
        .iter()
        .filter(|row| row.slug == service.as_str())
        .min_by_key(|row| (service_row_rank(row), row.id.as_str()))?;
    (rows.iter().filter(|row| row.id == selected.id).count() == 1).then_some(selected)
}
