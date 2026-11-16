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
    pub partition_key: String,
    #[serde(default)]
    pub consumer_id: Option<String>,
    pub expected_version: u64,
    pub expected_agent_id: String,
}
#[derive(Serialize, ToSchema)]
pub struct PartitionResetResponse {
    pub consumer_id: String,
    pub transport_conversation_id: String,
    pub reset: ConversationReset,
}

#[utoipa::path(post, path = "/v1/channels/{channel_id}/partition/resets", operation_id = "resetChannelPartition",
    tag = "channels", security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header), ("Idempotency-Key" = String, Header)),
    request_body = PartitionResetRequest,
    responses((status = 200, body = PartitionResetResponse), (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json")))]
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
        Err(ManagementError::Provider(ProviderError::Rejected { status: 409, code }))
            if REFUSALS.contains(&code.as_str()) =>
        {
            no_store(problem(StatusCode::CONFLICT, ProblemCode::Conflict, code))
        }
        Err(error) => management_error(error),
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
    let channel = parse_channel_id(raw)?;
    let loaded = management::load(state, creator, &channel).await?;
    if (loaded.definition.owner.kind == NyxIdIdentityKind::Organization) != creator.organization {
        return Err(ManagementError::NotFound);
    }
    if loaded.record.version != body.expected_version {
        return Err(ManagementError::Conflict {
            expected: body.expected_version,
            actual: Some(loaded.record.version),
        });
    }
    if body.partition_key.is_empty()
        || body.partition_key.len() > 8192
        || body.expected_agent_id.is_empty()
        || body.expected_agent_id.len() > 128
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/",
            "invalid partition key or expected Agent",
        )));
    }
    let consumer_id = body
        .consumer_id
        .as_deref()
        .map(ConsumerId::parse)
        .transpose()
        .map_err(ManagementError::Invalid)?;
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
    let credential = consumer.credential.ok_or_else(|| {
        ManagementError::Provider(ProviderError::Unavailable(
            "provider credential unavailable".into(),
        ))
    })?;
    let operation = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                creator.organization,
                &creator.subject,
                channel.as_str(),
                key
            ))
            .expect("tuple")
        )
    );
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&body).expect("request"))
    );
    let result = state.providers.reset_conversation(&consumer.provider.slug, &consumer.provider.binding_id, &conversation, credential, &operation, serde_json::json!({
        "expected_agent_id": body.expected_agent_id, "owner": creator.subject, "organization": creator.organization,
        "request_digest": fingerprint, "idempotency_binding_id": loaded.definition.provider.binding_id,
    })).await.map_err(ManagementError::Provider)?;
    // Do not turn a committed reset into a new request if channel metadata changed.
    // The result describes the exact authorized version supplied above.
    Ok(PartitionResetResponse {
        consumer_id: consumer.id.to_string(),
        transport_conversation_id: conversation.to_string(),
        reset: result,
    })
}
