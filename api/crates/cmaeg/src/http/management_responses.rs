//! Public projections and sanitized management refusals.
use super::{
    management::{ChannelResponse, EndpointResponse, ProviderResponse},
    problems::{no_store, problem},
};
use crate::channels::management::{ManagementError, ManagementState};
use axum::{
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use cmaeg_core::{ids::ChannelId, problem::ProblemCode};
use subtle::ConstantTimeEq;
pub(super) fn channel_response(
    state: &ManagementState,
    channel_id: &ChannelId,
    definition: &cmaeg_core::channel::ChannelDefinition,
    version: u64,
    provider: ProviderResponse,
    record_id: &cmaeg_core::ids::RecordId,
) -> Result<ChannelResponse, ManagementError> {
    Ok(ChannelResponse {
        channel_id: channel_id.to_string(),
        record_id: record_id.to_string(),
        version,
        provider,
        endpoints: EndpointResponse {
            nyxid_callback_url: format!("{}/v1/callbacks/{}", state.public_url, channel_id),
            telegram_webhook_url: definition
                .sources
                .iter()
                .any(|source| matches!(source, cmaeg_core::channel::Source::Telegram { .. }))
                .then(|| format!("{}/v1/telegram/{}", state.public_url, channel_id)),
            custom_event_url: format!("{}/v1/channels/{}/events", state.public_url, channel_id),
        },
        definition: super::schema::PublicChannelDefinition::new(definition, version),
    })
}

pub(super) fn provider_response(
    definition: &cmaeg_core::channel::ChannelDefinition,
) -> ProviderResponse {
    ProviderResponse {
        slug: definition.provider.slug.to_string(),
        binding_id: definition.provider.binding_id.to_string(),
        level: definition.provider.level,
        honoured: Vec::new(),
        refused: Vec::new(),
    }
}

pub(super) fn parse_channel_id(raw: &str) -> Result<ChannelId, ManagementError> {
    ChannelId::new(raw.to_owned()).map_err(ManagementError::Invalid)
}

pub(super) fn idempotency_key(headers: &HeaderMap) -> Result<Option<String>, ManagementError> {
    let Some(value) = headers.get("idempotency-key") else {
        return Ok(None);
    };
    let invalid = || {
        ManagementError::Invalid(cmaeg_core::ValidationError::new(
            "/headers/Idempotency-Key",
            "must contain 1..=256 ASCII bytes",
        ))
    };
    let value = value.to_str().map_err(|_| invalid())?;
    if value.is_empty() || value.len() > 256 {
        return Err(invalid());
    }
    Ok(Some(value.to_owned()))
}

pub(super) fn service_authorized(headers: &HeaderMap, expected: &str) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return false;
    };
    token.as_bytes().ct_eq(expected.as_bytes()).into()
}

pub(super) fn management_error(error: ManagementError) -> Response {
    let (status, code, detail) = match error {
        ManagementError::Invalid(error) => (
            StatusCode::BAD_REQUEST,
            ProblemCode::InvalidRequest,
            error.to_string(),
        ),
        ManagementError::Provider(crate::providers::ProviderError::UnknownProvider(detail))
        | ManagementError::Provider(crate::providers::ProviderError::InvalidRequest(detail)) => {
            (StatusCode::BAD_REQUEST, ProblemCode::InvalidRequest, detail)
        }
        ManagementError::Provider(crate::providers::ProviderError::Rejected { .. }) => (
            StatusCode::BAD_GATEWAY,
            ProblemCode::ProviderRejected,
            "provider rejected the channel request".into(),
        ),
        ManagementError::Provider(crate::providers::ProviderError::Busy { retry_after }) => (
            StatusCode::SERVICE_UNAVAILABLE,
            ProblemCode::ProviderBusy,
            format!("provider is busy; retry_after={retry_after:?}"),
        ),
        ManagementError::Provider(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            ProblemCode::ProviderUnavailable,
            "provider request failed".into(),
        ),
        ManagementError::Record(crate::records::RecordError::Conflict { expected, actual }) => (
            StatusCode::CONFLICT,
            ProblemCode::Conflict,
            format!("expected version {expected}, actual {actual:?}"),
        ),
        ManagementError::Record(crate::records::RecordError::NotFound)
        | ManagementError::NotFound => (
            StatusCode::NOT_FOUND,
            ProblemCode::ChannelNotFound,
            "The channel does not exist.".into(),
        ),
        ManagementError::Record(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            ProblemCode::ProviderUnavailable,
            format!("record store: {error}"),
        ),
        ManagementError::Conflict { expected, actual } => (
            StatusCode::CONFLICT,
            ProblemCode::Conflict,
            format!("expected version {expected}, actual {actual:?}"),
        ),
        ManagementError::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            ProblemCode::NotAuthenticated,
            "Authentication is required.".into(),
        ),
        ManagementError::Internal(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            ProblemCode::Internal,
            "channel operation failed".into(),
        ),
    };
    no_store(problem(status, code, detail))
}
