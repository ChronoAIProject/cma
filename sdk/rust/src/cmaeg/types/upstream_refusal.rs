//! Closed provider refusal diagnostics. Invalid wire values are rejected before
//! exposing the problem to a caller, with errors that omit the rejected value.
use serde::{Deserialize, Deserializer, Serialize, de::Error};

/// CMAEG's bounded provider refusal extension.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct UpstreamRefusal {
    /// The provider operation that refused startup.
    pub operation: UpstreamRefusalOperation,
    /// Actual provider HTTP status, independently of the outer gateway status.
    #[serde(deserialize_with = "http_status")]
    pub status: u16,
    /// Gateway allowlisted code, including the generic fallback.
    pub code: UpstreamRefusalCode,
}

/// The only provider operations described by the refusal extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", try_from = "String")]
pub enum UpstreamRefusalOperation {
    /// Response admission.
    Responses,
    /// Conversation preparation.
    Conversation,
}

impl TryFrom<String> for UpstreamRefusalOperation {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "responses" => Ok(Self::Responses),
            "conversation" => Ok(Self::Conversation),
            _ => Err("invalid upstream refusal operation"),
        }
    }
}

/// Safe codes emitted by the gateway; arbitrary upstream values are excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", try_from = "String")]
pub enum UpstreamRefusalCode {
    /// Provider declined the supplied media source.
    MediaSourceRefused,
    /// Partition Agent is missing.
    PartitionAgentMissing,
    /// Partition Agent cannot accept work.
    PartitionAgentUnavailable,
    /// Command admission conflicts with retained state.
    AgentCommandAdmissionConflict,
    /// Command key conflicts with an existing command.
    AgentCommandKeyConflict,
    /// Binding is not ready.
    BindingNotReady,
    /// Binding resources are unavailable.
    BindingResourcesUnavailable,
    /// Command authority is required.
    CommandAuthorityRequired,
    /// Response access is forbidden.
    ResponseForbidden,
    /// Response is missing.
    ResponseNotFound,
    /// Requested model does not match.
    ModelMismatch,
    /// Gateway-normalized fallback for every unrecognized provider code.
    Unrecognized,
}

impl TryFrom<String> for UpstreamRefusalCode {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "media_source_refused" => Ok(Self::MediaSourceRefused),
            "partition_agent_missing" => Ok(Self::PartitionAgentMissing),
            "partition_agent_unavailable" => Ok(Self::PartitionAgentUnavailable),
            "agent_command_admission_conflict" => Ok(Self::AgentCommandAdmissionConflict),
            "agent_command_key_conflict" => Ok(Self::AgentCommandKeyConflict),
            "binding_not_ready" => Ok(Self::BindingNotReady),
            "binding_resources_unavailable" => Ok(Self::BindingResourcesUnavailable),
            "command_authority_required" => Ok(Self::CommandAuthorityRequired),
            "response_forbidden" => Ok(Self::ResponseForbidden),
            "response_not_found" => Ok(Self::ResponseNotFound),
            "model_mismatch" => Ok(Self::ModelMismatch),
            "unrecognized" => Ok(Self::Unrecognized),
            _ => Err("invalid upstream refusal code"),
        }
    }
}

fn http_status<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u16, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    value
        .as_u64()
        .filter(|status| (100..=599).contains(status))
        .map(|status| status as u16)
        .ok_or_else(|| {
            D::Error::custom("upstream HTTP status must be an integer from 100 through 599")
        })
}
