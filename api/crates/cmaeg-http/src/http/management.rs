//! HTTP responses for channel management operations.
use super::problems::{no_store, problem_document};
use axum::{
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::ids::ChannelId;
use cmaeg_engine::management::{ChannelTest, ManagementFailure};

pub async fn test_channel(operation: &dyn ChannelTest, raw_id: String) -> Response {
    let channel_id = match ChannelId::new(raw_id) {
        Ok(channel_id) => channel_id,
        Err(error) => return failure_response(ManagementFailure::invalid_request(error)),
    };
    match operation.test_channel(channel_id).await {
        Ok(result) => no_store(Json(result).into_response()),
        Err(error) => failure_response(error),
    }
}

pub fn failure_response(error: ManagementFailure) -> Response {
    let mut response = no_store(problem_document(error.problem));
    response.extensions_mut().insert(error.stage);
    response
}
