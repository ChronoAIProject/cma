use super::{
    management_responses::{management_error, parse_channel_id},
    problems::no_store,
};
use crate::{
    auth::Creator,
    channels::management::{self, ManagementError, ManagementState},
    providers::{ConversationInspection, ProviderError},
};
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::{
    ids::{compact_partition_key, ConsumerId, ConversationId},
    ValidationError,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct PartitionQuery {
    pub partition_key: String,
    pub consumer_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct PartitionInspection {
    pub consumer_id: String,
    pub transport_conversation_id: String,
    pub mapping: ConversationInspection,
}

#[utoipa::path(get, path = "/v1/channels/{channel_id}/partition", operation_id = "inspectChannelPartition",
    tag = "channels", security(("creatorBearer" = [])),
    params(("channel_id" = String, Path), ("X-CMAEG-Owner" = Option<String>, Header), PartitionQuery),
    responses((status = 200, body = PartitionInspection),
        (status = "default", body = cmaeg_core::problem::Problem, content_type = "application/problem+json")))]
pub async fn inspect(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw): Path<String>,
    query: Result<Query<PartitionQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(query)) => query,
        Err(_) => {
            return management_error(ManagementError::Invalid(ValidationError::new(
                "/query",
                "partition_key and an optional consumer_id are required",
            )))
        }
    };
    match read(&state, &creator, &raw, query).await {
        Ok(value) => no_store(Json(value).into_response()),
        Err(error) => management_error(error),
    }
}

async fn read(
    state: &ManagementState,
    creator: &Creator,
    raw: &str,
    query: PartitionQuery,
) -> Result<PartitionInspection, ManagementError> {
    if query.partition_key.is_empty() || query.partition_key.len() > 8192 {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/partition_key",
            "must contain 1..=8192 bytes",
        )));
    }
    let channel_id = parse_channel_id(raw)?;
    let loaded = management::load(state, creator, &channel_id).await?;
    let consumer_id = query
        .consumer_id
        .as_deref()
        .map(ConsumerId::parse)
        .transpose()
        .map_err(ManagementError::Invalid)?;
    let consumer = loaded
        .definition
        .consumer(consumer_id.as_ref())
        .ok_or_else(|| {
            ManagementError::Invalid(ValidationError::new(
                "/consumer_id",
                "consumer is not configured",
            ))
        })?;
    if consumer.provider.slug.as_str() != "cma_codex"
        || consumer.pending
        || (consumer.provider.target.is_profile()
            && consumer
                .profile
                .environment
                .as_ref()
                .and_then(|environment| environment.get("session_affinity"))
                .and_then(serde_json::Value::as_str)
                == Some("source_user"))
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/consumer_id",
            "partition inspection requires an active cma_codex consumer with conversation affinity",
        )));
    }
    let conversation = if consumer.id.is_default() {
        ConversationId::derive(
            &state.master_keys,
            &channel_id,
            loaded.definition.partition.version,
            &query.partition_key,
        )
    } else {
        ConversationId::derive_for_consumer(
            &state.master_keys,
            &channel_id,
            loaded.definition.partition.version,
            consumer.id,
            &compact_partition_key(&query.partition_key),
        )
    };
    let credential = consumer.credential.ok_or_else(|| {
        ManagementError::Provider(ProviderError::Unavailable(
            "provider credential unavailable".into(),
        ))
    })?;
    let mapping = state
        .providers
        .inspect_conversation(
            &consumer.provider.slug,
            &consumer.provider.binding_id,
            &conversation,
            credential,
        )
        .await
        .map_err(ManagementError::Provider)?;
    let current = management::load(state, creator, &channel_id).await?;
    if current.record.version != loaded.record.version {
        return Err(ManagementError::Conflict {
            expected: loaded.record.version,
            actual: Some(current.record.version),
        });
    }
    Ok(PartitionInspection {
        consumer_id: consumer.id.to_string(),
        transport_conversation_id: conversation.to_string(),
        mapping,
    })
}
