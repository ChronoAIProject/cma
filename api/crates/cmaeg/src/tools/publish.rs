//! Bounded input and admission results for agent hand-offs.
use crate::http::consumer_admission::AdmissionStatus;
use cmaeg_core::{
    channel::ReplyChoice,
    ids::{ConsumerId, ConsumerName, ConversationId, EventId},
    problem::{Problem, ProblemCode},
    profile::canonical_json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::ToSchema;

pub const MAX_PUBLISH_TEXT: usize = 16 * 1024;
pub const MAX_PUBLISH_DATA: usize = 8 * 1024;

// No Debug: text, data and idempotency keys must never enter traces.
#[derive(Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishEventRequest {
    pub targets: Vec<String>,
    pub text: String,
    #[serde(default)]
    pub data: Option<Map<String, Value>>,
    #[serde(default, rename = "type")]
    pub event_type: Option<String>,
    #[serde(default)]
    pub reply: Option<ReplyChoice>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublishEventResponse {
    pub event_id: EventId,
    pub hop: u8,
    pub targets: Vec<PublishTarget>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublishTarget {
    pub consumer_id: ConsumerId,
    pub name: ConsumerName,
    pub conversation_id: ConversationId,
    pub status: AdmissionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
}

impl PublishEventRequest {
    /// Guard 5 runs only after caller authorization and publish permission.
    pub fn validate(&self, max_targets: u8) -> Result<(), Problem> {
        if self.targets.is_empty() || self.targets.len() > usize::from(max_targets) {
            return Err(invalid("targets must contain 1..=max_targets entries"));
        }
        if self.text.is_empty() {
            return Err(invalid("text must contain 1..=16384 UTF-8 bytes"));
        }
        if self.text.len() > MAX_PUBLISH_TEXT {
            return Err(oversized("text exceeds 16384 UTF-8 bytes"));
        }
        if self.data.as_ref().is_some_and(|data| {
            canonical_json(Value::Object(data.clone())).len() > MAX_PUBLISH_DATA
        }) {
            return Err(oversized("data exceeds 8192 canonical JSON bytes"));
        }
        if self.event_type.as_ref().is_some_and(|value| {
            !(1..=128).contains(&value.len())
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        }) {
            return Err(invalid("type must match [A-Za-z0-9._-]{1,128}"));
        }
        if self.idempotency_key.as_ref().is_some_and(|value| {
            !(1..=128).contains(&value.len())
                || !value.bytes().all(|byte| (32..=126).contains(&byte))
        }) {
            return Err(invalid(
                "idempotency_key must contain 1..=128 printable ASCII bytes",
            ));
        }
        Ok(())
    }
}

pub(super) fn invalid(detail: &str) -> Problem {
    Problem::new(ProblemCode::InvalidRequest, 400, detail)
}
fn oversized(detail: &str) -> Problem {
    Problem::new(ProblemCode::PayloadTooLarge, 413, detail)
}
