//! Validate the runtime identity before admitting a conversation attachment.
use super::output;
use crate::AdapterError;
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;

#[derive(Deserialize)]
struct Ready {
    t: String,
    #[serde(rename = "wireVersion")]
    wire_version: u32,
    lanes: Vec<String>,
    #[serde(rename = "agentId")]
    agent_id: String,
    #[serde(default, rename = "outputAcknowledgement")]
    output_acknowledgement: Option<output::OutputCapability>,
}

pub(super) fn validate_ready(
    message: Message,
    expected_agent: &str,
) -> crate::Result<Option<uuid::Uuid>> {
    let Message::Text(text) = message else {
        return Err(AdapterError::Rejected(
            "runtime sent a non-text Ready declaration".into(),
        ));
    };
    let ready: Ready = serde_json::from_str(&text)
        .map_err(|_| AdapterError::Rejected("runtime Ready declaration is malformed".into()))?;
    if ready.t != "ready"
        || ready.wire_version != 1
        || ready.agent_id != expected_agent
        || !ready.lanes.iter().any(|lane| lane == "ctl")
        || !ready.lanes.iter().any(|lane| lane == "conv")
    {
        return Err(AdapterError::Rejected(
            "runtime Ready declaration does not match the requested agent".into(),
        ));
    }
    ready
        .output_acknowledgement
        .map(|capability| capability.validate())
        .transpose()
}
