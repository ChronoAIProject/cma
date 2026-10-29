//! Public projections and sanitized management refusals.
use super::{
    management::{ChannelListItem, ChannelResponse, EndpointResponse, ProviderResponse},
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
        endpoints: EndpointResponse {
            native_lark_webhook_url: definition
                .sources
                .iter()
                .any(|s| matches!(s, cmaeg_core::channel::Source::NativeLark(_)))
                .then(|| {
                    format!(
                        "{}/v1/native-lark/{}",
                        state.advertised_api_url(),
                        channel_id
                    )
                }),
            whatsapp_webhook_url: definition
                .sources
                .iter()
                .any(|source| matches!(source, cmaeg_core::channel::Source::Whatsapp { .. }))
                .then(|| format!("{}/v1/whatsapp/{}", state.advertised_api_url(), channel_id)),
            nyxid_callback_url: format!(
                "{}/v1/callbacks/{}",
                state.advertised_api_url(),
                channel_id
            ),
            telegram_webhook_url: definition
                .sources
                .iter()
                .any(|source| matches!(source, cmaeg_core::channel::Source::Telegram { .. }))
                .then(|| format!("{}/v1/telegram/{}", state.advertised_api_url(), channel_id)),
            custom_event_url: format!(
                "{}/v1/channels/{}/events",
                state.advertised_api_url(),
                channel_id
            ),
        },
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
        ManagementError::Provider(crate::providers::ProviderError::Rejected { status: 409, code })
            if code == "binding_resources_unavailable" => (
            StatusCode::CONFLICT,
            ProblemCode::ProviderRejected,
            "The channel's sandbox or workspace is missing or removed. Reconnect the channel to an available agent.".into(),
        ),
        ManagementError::Provider(crate::providers::ProviderError::Rejected { status: 409, code })
            if code == "binding_reconfiguration_unsupported" => (
            StatusCode::CONFLICT,
            ProblemCode::ProviderRejected,
            "This channel's existing agent configuration is pinned. Changing its service scope, skills, prompt, model, environment, or conversation affinity through a channel update is not supported. The provider binding, credentials, and retained work have not been changed. Keep the current profile to save other channel settings, and rotate credentials separately using that profile. Retrying with the same key will not apply this change.".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn known_removed_binding_has_actionable_conflict_instead_of_generic_bad_gateway() {
        let response = management_error(ManagementError::Provider(
            crate::providers::ProviderError::Rejected {
                status: 409,
                code: "binding_resources_unavailable".into(),
            },
        ));
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["detail"]
            .as_str()
            .unwrap()
            .contains("Reconnect the channel"));
    }

    #[tokio::test]
    async fn known_reconfiguration_refusal_is_actionable_and_keeps_its_conflict_status() {
        let response = management_error(ManagementError::Provider(
            crate::providers::ProviderError::Rejected {
                status: 409,
                code: "binding_reconfiguration_unsupported".into(),
            },
        ));
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers()["content-type"],
            "application/problem+json"
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "provider_rejected");
        assert_eq!(body["status"], 409);
        let detail = body["detail"].as_str().unwrap();
        assert!(detail.contains("service scope"));
        assert!(detail.contains("have not been changed"));
        assert!(detail.contains("Keep the current profile"));
    }

    #[tokio::test]
    async fn unrecognized_refusals_do_not_expose_provider_error_text() {
        for (status, code) in [
            (409, "private-provider-credential-must-not-leak"),
            (400, "binding_reconfiguration_unsupported"),
        ] {
            let response = management_error(ManagementError::Provider(
                crate::providers::ProviderError::Rejected {
                    status,
                    code: code.into(),
                },
            ));
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["detail"], "provider rejected the channel request");
            assert!(!String::from_utf8_lossy(&bytes).contains(code));
        }
    }

    #[test]
    fn unrelated_provider_conflicts_keep_the_existing_error_mapping() {
        let response = management_error(ManagementError::Provider(
            crate::providers::ProviderError::Rejected {
                status: 409,
                code: "another_provider_failure".into(),
            },
        ));
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }
}
