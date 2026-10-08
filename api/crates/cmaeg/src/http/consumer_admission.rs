//! Public admission results for each target of one routed event.
use cmaeg_core::ids::{ConsumerId, ConsumerName, ConversationId};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConsumerAdmission {
    pub consumer_id: ConsumerId,
    pub name: ConsumerName,
    pub conversation_id: ConversationId,
    pub status: AdmissionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionStatus {
    Accepted,
    Deferred,
    Refused,
}
