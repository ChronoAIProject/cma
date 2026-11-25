//! Public projections and sanitized management refusals.
use super::{
    management::{ChannelListItem, ChannelResponse, ProviderResponse},
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
        native_discord_activation: crate::discord::activation::view(state, definition),
        thread_contract_versions: if state.thread_contract_writes_enabled
            && cmaeg_core::thread::supported(definition)
        {
            vec![1]
        } else {
            Vec::new()
        },
        channel_id: channel_id.to_string(),
        record_id: record_id.to_string(),
        version,
        provider,
        endpoints: super::channel_endpoints::channel_endpoints(state, channel_id, definition),
        definition: super::schema::PublicChannelDefinition::new(definition, version),
    })
}

/// Show every valid owned record without manufacturing a replacement sealed ID.
pub(super) fn channel_list_item(
    state: &ManagementState,
    record: &crate::channels::management::RecordedChannel,
) -> Result<ChannelListItem, ManagementError> {
    if !record.valid {
        return Err(ManagementError::Internal(
            "retained channel is invalid".into(),
        ));
    }
    let definition = &record.definition;
    let known = definition
        .creation
        .as_ref()
        .map(|creation| {
            crate::console::channel_ids::opens_to(
                &creation.channel_id,
                &definition.record_id,
                &state.master_keys,
                &state.records,
            )
            .map_err(|_| ManagementError::Internal("retained channel ID is invalid".into()))?;
            channel_response(
                state,
                &creation.channel_id,
                definition,
                record.version,
                provider_response(definition),
                &definition.record_id,
            )
        })
        .transpose()?;
    Ok(ChannelListItem {
        native_discord_activation: crate::discord::activation::view(state, definition),
        channel_id: known.as_ref().map(|response| response.channel_id.clone()),
        endpoints: known.map(|response| response.endpoints),
        record_id: definition.record_id.to_string(),
        version: record.version,
        provider: provider_response(definition),
        definition: super::schema::PublicChannelDefinition::new(definition, record.version),
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

/// Why a provider request failed. Only closed phrases and an HTTP status leave
/// the gateway: transport text can name internal addresses.
pub(super) fn provider_failure_detail(error: &crate::providers::ProviderError) -> String {
    use crate::providers::ProviderError;
    let reason = match error {
        ProviderError::Capacity => {
            "the gateway's provider concurrency is exhausted; retry later".to_owned()
        }
        ProviderError::ConversationMissing => "the provider conversation is missing".to_owned(),
        ProviderError::Unavailable(detail) => match detail.as_str() {
            "provider request timed out" => "the provider request timed out".to_owned(),
            "provider transport failed" => "the provider was unreachable".to_owned(),
            other => match other
                .strip_prefix("provider returned HTTP ")
                .and_then(|status| status.parse::<u16>().ok())
                .filter(|status| (100..=599).contains(status))
            {
                Some(status) => format!("the provider answered HTTP {status}"),
                None => "the provider or its service credential was unavailable".to_owned(),
            },
        },
        ProviderError::ResponseMalformed(_) => "the provider response is malformed".to_owned(),
        ProviderError::Sse(_) => "the provider stream ended before a terminal answer".to_owned(),
        _ => return "provider request failed".to_owned(),
    };
    format!("provider request failed: {reason}")
}

/// Name the provider's answer without echoing arbitrary provider text.
fn release_refused_detail(status: u16, code: &str) -> String {
    let named = !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    let answer = if named {
        format!("HTTP {status} {code}")
    } else {
        format!("HTTP {status}")
    };
    format!(
        "The provider refused to release this channel's binding ({answer}). The channel was not deleted and keeps its version. A channel whose creation never became ready cannot process events. The provider must release the binding, or report it absent with 404, before deletion can complete."
    )
}

#[cfg(test)]
#[path = "management_release_tests.rs"]
mod release_tests;

pub(super) fn management_error(error: ManagementError) -> Response {
    use crate::telemetry::http_failure::FailureStage;
    let stage = match &error {
        ManagementError::Provider(_) | ManagementError::ReleaseRefused { .. } => {
            FailureStage::ChannelProvider
        }
        ManagementError::Record(_) | ManagementError::NotFound => FailureStage::ChannelRecord,
        ManagementError::Invalid(_) => FailureStage::RequestValidation,
        _ => FailureStage::ChannelOperation,
    };
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
        ManagementError::Provider(crate::providers::ProviderError::Rejected { status: 409, code })
            if code == "binding_resources_unavailable" => (
            StatusCode::CONFLICT,
            ProblemCode::ProviderRejected,
            "The channel's sandbox or workspace is missing or removed. Reconnect the channel to an available agent.".into(),
        ),
        ManagementError::Provider(crate::providers::ProviderError::Rejected { status: 409, code })
            if code == "binding_skill_version_update_unsupported" => (
            StatusCode::CONFLICT,
            ProblemCode::Conflict,
            "The selected skill version cannot be changed through a channel profile update. Keep the channel profile unchanged and use the existing Agent's skills endpoint (POST /api/v1/agents/{id}/skills) when upgrades are enabled and supported.".into(),
        ),
        ManagementError::Provider(crate::providers::ProviderError::Rejected { status: 409, code })
            if code == "binding_reconfiguration_unsupported" => (
            StatusCode::CONFLICT,
            ProblemCode::ProviderRejected,
            "This channel's existing agent configuration is pinned. Changing its service scope, skills, prompt, model, environment, or conversation affinity through a channel update is not supported. To use a different service scope, create a new binding with the required scope. The provider binding, credentials, and retained work have not been changed. Keep the current profile to save other channel settings, and rotate credentials separately using that profile. Retrying with the same key will not apply this change.".into(),
        ),
        ManagementError::Provider(crate::providers::ProviderError::Rejected { status: 404, code })
            if code == "binding_not_found" => (
            StatusCode::NOT_FOUND,
            ProblemCode::ProviderRejected,
            "The binding, conversation, or target Agent is not available to this channel owner.".into(),
        ),
        ManagementError::Provider(crate::providers::ProviderError::Rejected { status: 409, code })
            if code == "binding_not_ready" => (
            StatusCode::CONFLICT,
            ProblemCode::ProviderRejected,
            "The binding or target Agent is not ready. Inspect the target's lifecycle state and the binding configuration before retrying.".into(),
        ),
        ManagementError::Provider(crate::providers::ProviderError::Rejected { status: 400, code })
            if code == "invalid_binding" => (
            StatusCode::BAD_REQUEST,
            ProblemCode::ProviderRejected,
            "The provider refused the binding profile or target. Check the channel configuration before retrying.".into(),
        ),
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
        // Closed details only: a transport error can name internal addresses.
        ManagementError::Provider(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            ProblemCode::ProviderUnavailable,
            provider_failure_detail(&error),
        ),
        ManagementError::Record(crate::records::RecordError::Conflict { expected, actual }) => (
            StatusCode::CONFLICT,
            ProblemCode::Conflict,
            format!("expected version {expected}, actual {actual:?}"),
        ),
        ManagementError::Record(crate::records::RecordError::GuardConflict {
            scope: "channel_create",
            ..
        }) => (
            StatusCode::CONFLICT,
            ProblemCode::Conflict,
            "channel_create_fenced".into(),
        ),
        ManagementError::Record(crate::records::RecordError::GuardConflict {
            scope: "connection",
            ..
        }) => (
            StatusCode::CONFLICT,
            ProblemCode::Conflict,
            "connection_claim_changed".into(),
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
        ManagementError::CreationPending => (
            StatusCode::CONFLICT,
            ProblemCode::Conflict,
            crate::channels::creation::PENDING_DETAIL.into(),
        ),
        ManagementError::ReleaseRefused { status, code } => (
            StatusCode::CONFLICT,
            ProblemCode::ProviderRejected,
            release_refused_detail(status, &code),
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
    stage.response(no_store(problem(status, code, detail)))
}

#[cfg(test)]
#[path = "management_response_tests.rs"]
mod tests;

#[cfg(test)]
mod telemetry_tests {
    use super::*;
    use crate::{records::RecordError, telemetry::http_failure::FailureStage};

    #[test]
    fn record_and_provider_outages_have_distinct_stages() {
        for (error, stage) in [
            (
                ManagementError::Record(RecordError::NotFound),
                FailureStage::ChannelRecord,
            ),
            (
                ManagementError::Provider(crate::providers::ProviderError::Busy {
                    retry_after: None,
                }),
                FailureStage::ChannelProvider,
            ),
            (
                ManagementError::Internal("private-error-token".into()),
                FailureStage::ChannelOperation,
            ),
        ] {
            let response = management_error(error);
            assert_eq!(response.extensions().get::<FailureStage>(), Some(&stage));
        }
    }
}
