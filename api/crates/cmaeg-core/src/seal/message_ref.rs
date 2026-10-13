//! A confirmed destination, authenticated under its own token kind.
use crate::{
    ids::{ChannelId, ConsumerId, EventId},
    require_string, ValidationError,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageRefPayload {
    pub channel_id: ChannelId,
    pub event_id: EventId,
    pub platform_message_id: String,
    pub route_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_record_id: Option<String>,
    /// The consumer this reference authorizes; absent for the default
    /// consumer (never `Some("default")`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer: Option<ConsumerId>,
}
impl MessageRefPayload {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require_string(&self.platform_message_id, "/platform_message_id", 1, 512)?;
        require_string(&self.route_id, "/route_id", 1, 512)?;
        super::event_ref::validate_consumer(self.consumer.as_ref())
    }
}
