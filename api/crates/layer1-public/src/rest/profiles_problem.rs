use axum::{http::StatusCode, response::IntoResponse, response::Response};
use layer2_core::profiles::{MarketplaceError, ProfileStoreError};

use super::{problem_document, problem_response};

pub(super) enum ProfileProblem {
    Forbidden,
    NotFound,
    VersionConflict,
    Invalid { field: String, reason: String },
    ReferenceInvalid { path: String },
    PublicRefused { reason: String },
    InvalidPage,
    MarketplaceDisabled,
    StoreUnavailable,
}

impl ProfileProblem {
    pub(super) fn from_store(error: ProfileStoreError, reference_path: &str) -> Self {
        match error {
            ProfileStoreError::NotFound | ProfileStoreError::Protected => Self::NotFound,
            ProfileStoreError::Duplicate | ProfileStoreError::Conflict => Self::VersionConflict,
            ProfileStoreError::ReferenceUnavailable => Self::ReferenceInvalid {
                path: reference_path.to_owned(),
            },
            ProfileStoreError::Validation(error) => Self::Invalid {
                field: error.field.to_owned(),
                reason: error.code.to_owned(),
            },
            ProfileStoreError::Unavailable(_) | ProfileStoreError::Corrupt(_) => {
                Self::StoreUnavailable
            }
        }
    }

    pub(super) fn from_marketplace(error: MarketplaceError, public_change: bool) -> Self {
        match error {
            MarketplaceError::Disabled => Self::MarketplaceDisabled,
            MarketplaceError::Unpublished if public_change => Self::PublicRefused {
                reason: "profile_unpublished".into(),
            },
            MarketplaceError::NotFound
            | MarketplaceError::Forbidden
            | MarketplaceError::Unpublished => Self::NotFound,
            MarketplaceError::Validation(reason) if public_change => Self::PublicRefused { reason },
            MarketplaceError::Validation(reason) => Self::Invalid {
                field: "content".into(),
                reason,
            },
            MarketplaceError::InvalidPage => Self::InvalidPage,
            MarketplaceError::TrustConfirmationRequired { .. }
            | MarketplaceError::TrustConfirmationMismatch => Self::Invalid {
                field: "commands_digest".into(),
                reason: "trust_confirmation_invalid".into(),
            },
            MarketplaceError::Unavailable(_) | MarketplaceError::Corrupt(_) => {
                Self::StoreUnavailable
            }
        }
    }
}

impl IntoResponse for ProfileProblem {
    fn into_response(self) -> Response {
        let (status, slug, detail, code, extra) = match self {
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "This caller is not allowed to manage profiles.",
                "forbidden",
                None,
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "profile-not-found",
                "No profile matches this request.",
                "profile_not_found",
                None,
            ),
            Self::VersionConflict => (
                StatusCode::CONFLICT,
                "profile-version-conflict",
                "The profile draft changed before this operation completed.",
                "profile_version_conflict",
                None,
            ),
            Self::Invalid { field, reason } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "profile-invalid",
                "The profile does not satisfy its validation contract.",
                "profile_invalid",
                Some(("field", field, Some(("reason", reason)))),
            ),
            Self::ReferenceInvalid { path } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "profile-reference-invalid",
                "A referenced profile revision is unavailable to this owner.",
                "profile_reference_invalid",
                Some(("path", path, None)),
            ),
            Self::PublicRefused { reason } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "profile-public-refused",
                "This profile cannot be made public.",
                "profile_public_refused",
                Some(("reason", reason, None)),
            ),
            Self::InvalidPage => (
                StatusCode::BAD_REQUEST,
                "invalid-profile-page",
                "The profile page parameters are not valid.",
                "invalid_profile_page",
                None,
            ),
            Self::MarketplaceDisabled => (
                StatusCode::SERVICE_UNAVAILABLE,
                "marketplace-disabled",
                "The profile marketplace is disabled by the operator.",
                "marketplace_disabled",
                None,
            ),
            Self::StoreUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "profile-store-unavailable",
                "The profile store could not complete this operation.",
                "profile_store_unavailable",
                None,
            ),
        };
        let mut body = problem_document(status, slug, detail, code);
        if let (Some(object), Some((name, value, nested))) = (body.as_object_mut(), extra) {
            object.insert(name.into(), value.into());
            if let Some((nested_name, nested_value)) = nested {
                object.insert(nested_name.into(), nested_value.into());
            }
        }
        problem_response(status, body)
    }
}
