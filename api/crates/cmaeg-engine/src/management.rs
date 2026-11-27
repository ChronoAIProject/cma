//! Authorized channel management operation contracts.
use cmaeg_core::{
    channel::ConformanceLevel,
    ids::ChannelId,
    problem::{Problem, ProblemCode},
    ValidationError,
};
use serde::Serialize;
use std::{future::Future, pin::Pin};
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagementFailureStage {
    RequestValidation,
    ChannelRecord,
    ChannelProvider,
    ChannelOperation,
}

pub struct ManagementFailure {
    pub problem: Problem,
    pub stage: ManagementFailureStage,
}

impl ManagementFailure {
    pub fn invalid_request(error: ValidationError) -> Self {
        Self {
            problem: Problem::new(ProblemCode::InvalidRequest, 400, error.to_string()),
            stage: ManagementFailureStage::RequestValidation,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TestChannelResponse {
    pub accepted_after_ms: u64,
    pub first_text: String,
    pub level: ConformanceLevel,
}

/// One complete test operation bound to the authenticated creator by its implementation.
pub trait ChannelTest: Send + Sync {
    fn test_channel(
        &self,
        channel_id: ChannelId,
    ) -> Pin<Box<dyn Future<Output = Result<TestChannelResponse, ManagementFailure>> + Send + '_>>;
}
