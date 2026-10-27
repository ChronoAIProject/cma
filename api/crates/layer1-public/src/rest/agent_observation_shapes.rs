use serde::Serialize;
use utoipa::ToSchema;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub(crate) struct AgentRuntimeMetadataView {
    #[serde(skip_serializing_if = "Option::is_none")]
    working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    description_source: Option<layer2_core::sandbox::AgentDescriptionSource>,
    description_is_setup_preview: bool,
}
impl AgentRuntimeMetadataView {
    pub(crate) fn from_agent(agent: &layer2_core::sandbox::Agent) -> Option<Self> {
        let metadata = &agent.conversation.observations.metadata;
        (metadata.working_directory.is_some() || metadata.description.is_some()).then(|| Self {
            working_directory: metadata.working_directory.clone(),
            description: metadata.description.clone(),
            description_source: metadata.description_source,
            description_is_setup_preview: metadata.description_is_setup_preview,
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub(crate) struct AgentTurnUsageView {
    turn_sequence: u64,
    input_tokens: u64,
    output_tokens: u64,
}
impl From<layer2_core::sandbox::AgentTurnUsage> for AgentTurnUsageView {
    fn from(value: layer2_core::sandbox::AgentTurnUsage) -> Self {
        Self {
            turn_sequence: value.turn_sequence,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
        }
    }
}

impl From<layer2_core::sandbox::AgentRuntimeMetadata> for AgentRuntimeMetadataView {
    fn from(value: layer2_core::sandbox::AgentRuntimeMetadata) -> Self {
        Self {
            working_directory: value.working_directory,
            description: value.description,
            description_source: value.description_source,
            description_is_setup_preview: value.description_is_setup_preview,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::sandbox::{AgentDescriptionSource, AgentRuntimeMetadata};

    #[test]
    fn observation_public_metadata_keeps_description_and_provenance_together() {
        let view = AgentRuntimeMetadataView::from(AgentRuntimeMetadata {
            description: Some("Setup preview".into()),
            description_source: Some(AgentDescriptionSource::PromptPreview),
            description_is_setup_preview: true,
            ..Default::default()
        });
        let value = serde_json::to_value(view).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "description": "Setup preview",
                "description_source": "prompt_preview",
                "description_is_setup_preview": true
            })
        );
    }
}
