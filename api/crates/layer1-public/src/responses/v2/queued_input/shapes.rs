use super::super::controls::shapes::V2CommandStatus;
use layer2_core::sandbox::agent_io::{QueuedInputStatusView, QueuedInputView};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct V2QueuedInputRequest {
    pub source_command_id: String,
    pub expected_command_revision: u64,
    pub text: String,
}
#[derive(Serialize, ToSchema)]
pub struct V2QueuedInputItem {
    pub response: V2CommandStatus,
    pub command_revision: u64,
    pub text: String,
    pub editable: bool,
}
impl From<QueuedInputView> for V2QueuedInputItem {
    fn from(value: QueuedInputView) -> Self {
        Self {
            response: value.response.into(),
            command_revision: value.command_revision,
            text: value.text,
            editable: value.editable,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub struct V2QueuedInputPage {
    pub object: &'static str,
    pub agent_id: String,
    pub data: Vec<V2QueuedInputItem>,
    pub next_after: Option<u64>,
}
#[derive(Serialize, ToSchema)]
pub struct V2QueuedInputTarget {
    pub response_id: String,
    pub source_command_id: String,
    pub expected_command_revision: u64,
}
#[derive(Serialize, ToSchema)]
pub struct V2QueuedInput {
    pub object: &'static str,
    pub edit: V2CommandStatus,
    pub response: V2CommandStatus,
    pub target: V2QueuedInputTarget,
    pub command_revision: u64,
    pub text: String,
}
impl From<QueuedInputStatusView> for V2QueuedInput {
    fn from(value: QueuedInputStatusView) -> Self {
        Self {
            object: "cma.response.queued_input",
            edit: value.edit.into(),
            response: value.response.into(),
            target: V2QueuedInputTarget {
                response_id: value.binding.target.response_id.to_string(),
                source_command_id: value.binding.target.source_command_id.to_string(),
                expected_command_revision: value.binding.target.expected_command_revision,
            },
            command_revision: value.binding.command_revision,
            text: value.binding.text,
        }
    }
}
