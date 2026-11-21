use serde::{Deserialize, Serialize};

pub use layer3_adapter::ConversationDescriptionSource as AgentDescriptionSource;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRuntimeMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_source: Option<AgentDescriptionSource>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub description_is_setup_preview: bool,
}

impl AgentRuntimeMetadata {
    pub(super) fn record_task_preview(&mut self, text: &str) -> Option<super::AgentLogKind> {
        if self
            .description
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
            && (self.description_source == Some(AgentDescriptionSource::ProviderTitle)
                || (self.description_source == Some(AgentDescriptionSource::PromptPreview)
                    && !self.description_is_setup_preview))
        {
            return None;
        }
        self.description = Some(
            super::AgentTitle::fallback(Some(text.trim()))
                .as_str()
                .to_owned(),
        );
        self.description_source = Some(AgentDescriptionSource::PromptPreview);
        self.description_is_setup_preview = false;
        Some(super::AgentLogKind::RuntimeMetadataChanged {
            metadata: self.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTurnUsage {
    pub turn_sequence: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A cumulative checkpoint survives runtime replacement and context compaction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentUsageCheckpoint {
    pub conversation_id: String,
    pub source_turn: String,
    pub source_input: u64,
    pub source_output: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub context_used: u64,
    pub context_window: Option<u64>,
    pub current_turn: Option<AgentTurnUsage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentObservations {
    pub metadata: AgentRuntimeMetadata,
    pub usage: Option<AgentUsageCheckpoint>,
}

impl AgentObservations {
    pub fn is_empty(&self) -> bool {
        self.metadata == AgentRuntimeMetadata::default() && self.usage.is_none()
    }
}
