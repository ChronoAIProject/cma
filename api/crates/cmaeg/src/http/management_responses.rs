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
mod tests {
    use super::*;

    #[tokio::test]
    async fn provider_failures_name_their_class_without_transport_text() {
        use crate::providers::ProviderError;
        for (error, detail) in [
            (
                ProviderError::Unavailable("provider returned HTTP 502".into()),
                "provider request failed: the provider answered HTTP 502",
            ),
            (
                ProviderError::Unavailable("provider request timed out".into()),
                "provider request failed: the provider request timed out",
            ),
            (
                ProviderError::Unavailable("provider transport failed".into()),
                "provider request failed: the provider was unreachable",
            ),
            (
                ProviderError::Unavailable(
                    "error sending request for url (http://internal.example.svc/token)".into(),
                ),
                "provider request failed: the provider or its service credential was unavailable",
            ),
            (
                ProviderError::Capacity,
                "provider request failed: the gateway's provider concurrency is exhausted; retry later",
            ),
            (
                ProviderError::Sse("frame too large".into()),
                "provider request failed: the provider stream ended before a terminal answer",
            ),
        ] {
            let response = management_error(ManagementError::Provider(error));
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["code"], "provider_unavailable");
            assert_eq!(body["detail"], detail);
            assert!(!body["detail"].as_str().unwrap().contains("internal.example"));
        }
    }

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
    async fn unsupported_skill_version_update_stays_a_typed_conflict() {
        let response = management_error(ManagementError::Provider(
            crate::providers::ProviderError::Rejected {
                status: 409,
                code: "binding_skill_version_update_unsupported".into(),
            },
        ));
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "conflict");
        assert_eq!(
            body["detail"],
            "The selected skill version cannot be changed through a channel profile update. Keep the channel profile unchanged and use the existing Agent's skills endpoint (POST /api/v1/agents/{id}/skills) when upgrades are enabled and supported."
        );
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
        assert!(detail.contains("create a new binding with the required scope"));
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
