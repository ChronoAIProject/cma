use super::{
    management_responses::{management_error, parse_channel_id},
    problems::{no_store, problem},
};
use crate::{
    auth::Creator,
    channels::management::{self, ManagementError, ManagementState},
    providers::{ConversationReset, ProviderError},
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::{
    ids::ConsumerId, problem::ProblemCode, profile::NyxIdIdentityKind, ValidationError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PartitionResetRequest {
    /// Partition key containing 1..=8192 UTF-8 bytes.
    #[schema(min_length = 1, max_length = 8192)]
    pub partition_key: String,
    /// Omitted or null selects the default consumer. Explicit "default" has a distinct replay fingerprint.
    #[serde(default)]
    #[schema(pattern = "^(default|csm_[0-9a-f]{12})$")]
    pub consumer_id: Option<String>,
    #[schema(minimum = 1)]
    pub expected_version: u64,
    /// Expected deleted Agent identifier containing 1..=128 UTF-8 bytes.
    #[schema(min_length = 1, max_length = 128)]
    pub expected_agent_id: String,
}
#[derive(Serialize, Deserialize, ToSchema)]
pub struct PartitionResetResponse {
    pub consumer_id: String,
    pub transport_conversation_id: String,
    pub reset: ConversationReset,
}

#[utoipa::path(post, path = "/v1/channels/{channel_id}/partition/resets", operation_id = "resetChannelPartition",
    tag = "channels", security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header), ("Idempotency-Key" = String, Header, min_length = 1, max_length = 128, pattern = "^[!-~]+$", description = "Exactly one header with 1..=128 visible ASCII bytes. Reuse the key and complete request after an uncertain outcome.")),
    request_body = PartitionResetRequest,
    responses((status = 200, body = PartitionResetResponse), (status = 400, body = cmaeg_core::problem::Problem, content_type = "application/problem+json"), (status = 401, body = cmaeg_core::problem::Problem, content_type = "application/problem+json"), (status = 403, body = cmaeg_core::problem::Problem, content_type = "application/problem+json"), (status = 404, body = cmaeg_core::problem::Problem, content_type = "application/problem+json"), (status = 409, body = cmaeg_core::problem::Problem, content_type = "application/problem+json"), (status = 502, body = cmaeg_core::problem::Problem, content_type = "application/problem+json"), (status = 503, body = cmaeg_core::problem::Problem, content_type = "application/problem+json"), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json")))]
pub async fn reset(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw): Path<String>,
    headers: HeaderMap,
    body: Result<Json<PartitionResetRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(_) => return invalid("/", "invalid partition reset request"),
    };
    let key = headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    if headers.get_all("idempotency-key").iter().count() != 1
        || key.is_empty()
        || key.len() > 128
        || !key.bytes().all(|b| (33..=126).contains(&b))
    {
        return invalid(
            "/Idempotency-Key",
            "one key containing 1..=128 visible ASCII bytes is required",
        );
    }
    match perform(&state, &creator, &raw, key, body).await {
        Ok(value) => no_store(Json(value).into_response()),
        Err(error) => reset_error(error),
    }
}
fn reset_error(error: ManagementError) -> Response {
    match error {
        ManagementError::Provider(ProviderError::Rejected { status: 409, code })
            if REFUSALS.contains(&code.as_str()) =>
        {
            crate::telemetry::http_failure::FailureStage::ChannelProvider.response(no_store(
                problem(StatusCode::CONFLICT, ProblemCode::Conflict, code),
            ))
        }
        error => management_error(error),
    }
}
const REFUSALS: &[&str] = &[
    "partition_reset_disabled",
    "partition_reset_key_conflict",
    "partition_reset_existing_agent",
    "partition_reset_affinity_unsupported",
    "partition_reset_binding_not_ready",
    "partition_reset_mapping_retired",
    "partition_reset_agent_missing",
    "partition_reset_agent_not_deleted",
    "partition_reset_cleanup_pending",
    "partition_reset_parents_unavailable",
    "partition_reset_mapping_missing",
    "partition_reset_mapping_changed",
    "partition_reset_destination_exists",
    "partition_reset_capacity",
    "partition_reset_unsupported",
];
fn invalid(pointer: &str, message: &str) -> Response {
    management_error(ManagementError::Invalid(ValidationError::new(
        pointer, message,
    )))
}
async fn perform(
    state: &ManagementState,
    creator: &Creator,
    raw: &str,
    key: &str,
    body: PartitionResetRequest,
) -> Result<PartitionResetResponse, ManagementError> {
    if body.partition_key.is_empty()
        || body.partition_key.len() > 8192
        || body.expected_agent_id.is_empty()
        || body.expected_agent_id.len() > 128
        || body.expected_version == 0
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/",
            "invalid partition key, expected Agent, or channel version",
        )));
    }
    let consumer_id = body
        .consumer_id
        .as_deref()
        .map(ConsumerId::parse)
        .transpose()
        .map_err(ManagementError::Invalid)?;
    let channel = parse_channel_id(raw)?;
    let loaded = management::load(state, creator, &channel).await?;
    if (loaded.definition.owner.kind == NyxIdIdentityKind::Organization) != creator.organization {
        return Err(ManagementError::NotFound);
    }
    let consumer = loaded
        .definition
        .consumer(consumer_id.as_ref())
        .filter(|c| !c.pending)
        .ok_or_else(|| {
            ManagementError::Invalid(ValidationError::new(
                "/consumer_id",
                "consumer is not configured or is pending",
            ))
        })?;
    let conversation = super::partitions::conversation_id(
        &state.master_keys,
        &channel,
        loaded.definition.partition.version,
        consumer.id,
        &body.partition_key,
    );
    if consumer.provider.slug.as_str() != "cma_codex" {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/consumer_id",
            "partition reset requires a cma_codex consumer",
        )));
    }
    let operation = operation_id(
        creator.organization,
        &creator.subject,
        channel.as_str(),
        key,
    );
    let fingerprint = request_digest(&body);
    if loaded.definition.secrets.profile_revisions.is_some() && consumer.id.is_default() {
        let result = crate::channels::profile_assignment::reset_partition(
            state,
            creator,
            channel.as_str(),
            crate::channels::profile_assignment::ResetInput {
                consumer: consumer.id,
                conversation: &conversation,
                expected_version: body.expected_version,
                expected_agent_id: &body.expected_agent_id,
                operation_id: &operation,
                request_digest: &fingerprint,
            },
        )
        .await?;
        return Ok(PartitionResetResponse {
            consumer_id: consumer.id.to_string(),
            transport_conversation_id: conversation.to_string(),
            reset: result,
        });
    }
    if loaded.record.version != body.expected_version {
        return Err(ManagementError::Conflict {
            expected: body.expected_version,
            actual: Some(loaded.record.version),
        });
    }
    let scope = loaded
        .definition
        .consumer(None)
        .ok_or(ManagementError::NotFound)?;
    let result = state.providers.reset_conversation(&consumer.provider.slug, &consumer.provider.binding_id, &conversation, &operation, serde_json::json!({
        "expected_agent_id": body.expected_agent_id, "owner": creator.subject, "organization": creator.organization,
        "request_digest": fingerprint, "idempotency_binding_id": scope.provider.binding_id,
    })).await.map_err(ManagementError::Provider)?;
    // Do not turn a committed reset into a new request if channel metadata changed.
    // The result describes the exact authorized version supplied above.
    Ok(PartitionResetResponse {
        consumer_id: consumer.id.to_string(),
        transport_conversation_id: conversation.to_string(),
        reset: result,
    })
}

fn operation_id(organization: bool, owner: &str, channel: &str, key: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(organization, owner, channel, key)).expect("string tuple")
        )
    )
}
fn request_digest(body: &PartitionResetRequest) -> String {
    // Field order and explicit default selection are part of the retained replay contract.
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(body).expect("reset request"))
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn reset_fingerprints_normalize_json_but_preserve_explicit_default() {
        let first: PartitionResetRequest = serde_json::from_str(
            r#"{"partition_key":"fixture","expected_version":1,"expected_agent_id":"old"}"#,
        )
        .unwrap();
        let second: PartitionResetRequest = serde_json::from_str(r#"{ "expected_agent_id":"old", "consumer_id":null, "expected_version":1, "partition_key":"fixture" }"#).unwrap();
        assert_eq!(
            request_digest(&first),
            "260ad6d8265778775e3113ac14be6e7822ea97fed01cd84dd2dc4948548bf2cf"
        );
        assert_eq!(request_digest(&first), request_digest(&second));
        let mut explicit = second;
        explicit.consumer_id = Some("default".into());
        assert_ne!(request_digest(&first), request_digest(&explicit));
        let first = operation_id(false, "alice", "channel", "key");
        assert_eq!(
            first,
            "0291873ef8a1b7464eef84cb7ee2e743921582c829b8fa91ddb56e65b8bf5845"
        );
        for (organization, owner, channel, key) in [
            (true, "alice", "channel", "key"),
            (false, "bob", "channel", "key"),
            (false, "alice", "other", "key"),
            (false, "alice", "channel", "other"),
        ] {
            assert_ne!(first, operation_id(organization, owner, channel, key));
        }
    }
    #[tokio::test]
    async fn reset_refusals_keep_only_approved_codes_and_failure_stage() {
        for code in REFUSALS.iter().copied().chain(["private upstream code"]) {
            let response = reset_error(ManagementError::Provider(ProviderError::Rejected {
                status: 409,
                code: code.into(),
            }));
            assert_eq!(
                response
                    .extensions()
                    .get::<crate::telemetry::http_failure::FailureStage>(),
                Some(&crate::telemetry::http_failure::FailureStage::ChannelProvider)
            );
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert_eq!(
                response.status().as_u16(),
                if REFUSALS.contains(&code) { 409 } else { 502 }
            );
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                body["code"],
                json!(if REFUSALS.contains(&code) {
                    "conflict"
                } else {
                    "provider_rejected"
                })
            );
            assert_eq!(
                body["detail"],
                if REFUSALS.contains(&code) {
                    code
                } else {
                    "provider rejected the channel request"
                }
            );
        }
    }
    #[test]
    fn reset_public_schemas_validate_fixtures_and_reject_invalid_contracts() {
        let document: serde_json::Value =
            serde_json::from_str(&crate::openapi::snapshot()).unwrap();
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/sdk-contract.json")).unwrap();
        for name in [
            "PartitionResetRequest",
            "PartitionResetResponse",
            "ConversationReset",
            "ConversationInspection",
            "PartitionInspection",
        ] {
            let schema = json!({"$ref":format!("#/components/schemas/{name}"),"components":document["components"]});
            let validator = jsonschema::options()
                .with_draft(jsonschema::Draft::Draft202012)
                .build(&schema)
                .unwrap();
            for fixture in fixtures[name].as_array().unwrap() {
                match name {
                    "PartitionResetRequest" => {
                        serde_json::from_value::<PartitionResetRequest>(fixture.clone()).unwrap();
                    }
                    "PartitionResetResponse" => {
                        serde_json::from_value::<PartitionResetResponse>(fixture.clone()).unwrap();
                    }
                    "ConversationReset" => {
                        serde_json::from_value::<ConversationReset>(fixture.clone()).unwrap();
                    }
                    "ConversationInspection" => {
                        serde_json::from_value::<crate::providers::ConversationInspection>(
                            fixture.clone(),
                        )
                        .unwrap();
                    }
                    "PartitionInspection" => {
                        serde_json::from_value::<super::super::partitions::PartitionInspection>(
                            fixture.clone(),
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                assert!(
                    validator.is_valid(fixture),
                    "{name}: {:?}",
                    validator.validate(fixture)
                );
            }
            if name == "PartitionResetRequest" {
                for field in ["partition_key", "expected_agent_id", "expected_version"] {
                    let mut invalid = fixtures[name][0].clone();
                    invalid.as_object_mut().unwrap().remove(field);
                    assert!(!validator.is_valid(&invalid));
                    assert!(serde_json::from_value::<PartitionResetRequest>(invalid).is_err());
                }
                let mut invalid = fixtures[name][0].clone();
                invalid["profile"] = json!({});
                assert!(!validator.is_valid(&invalid));
                assert!(serde_json::from_value::<PartitionResetRequest>(invalid).is_err());
                let mut invalid = fixtures[name][0].clone();
                invalid["expected_version"] = json!(0);
                assert!(!validator.is_valid(&invalid));
            }
            if name == "ConversationReset" {
                for (field, value) in [
                    ("generation", json!(0)),
                    ("incarnation", json!(0)),
                    ("state", json!("ready")),
                ] {
                    let mut invalid = fixtures[name][0].clone();
                    invalid[field] = value;
                    assert!(!validator.is_valid(&invalid));
                }
            }
            if matches!(name, "ConversationInspection" | "PartitionInspection") {
                for value in [json!(-1), json!("1"), serde_json::Value::Null] {
                    let mut invalid = fixtures[name][0].clone();
                    if name == "ConversationInspection" {
                        invalid["incarnation"] = value;
                        assert!(
                            serde_json::from_value::<crate::providers::ConversationInspection>(
                                invalid.clone()
                            )
                            .is_err()
                        );
                    } else {
                        invalid["mapping"]["incarnation"] = value;
                        assert!(serde_json::from_value::<
                            super::super::partitions::PartitionInspection,
                        >(invalid.clone())
                        .is_err());
                    }
                    assert!(!validator.is_valid(&invalid));
                }
            }
        }
        assert!(
            !document["components"]["schemas"]["ConversationInspection"]["required"]
                .as_array()
                .unwrap()
                .contains(&json!("incarnation"))
        );
    }
    #[tokio::test]
    async fn reset_preserves_surrounding_management_error_projections() {
        for (status, code, expected) in [
            (404, "binding_not_found", 404),
            (400, "invalid_binding", 400),
            (409, "binding_not_ready", 409),
            (401, "private upstream authentication", 502),
            (400, "private upstream client detail", 502),
        ] {
            let response = reset_error(ManagementError::Provider(ProviderError::Rejected {
                status,
                code: code.into(),
            }));
            assert_eq!(response.status().as_u16(), expected);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            assert!(!String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("private upstream"));
        }
        for error in [
            ProviderError::Busy { retry_after: None },
            ProviderError::Capacity,
            ProviderError::Unavailable("private upstream".into()),
            ProviderError::ResponseMalformed("private upstream".into()),
        ] {
            let response = reset_error(ManagementError::Provider(error));
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            assert!(!String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("private upstream"));
        }
    }
}
