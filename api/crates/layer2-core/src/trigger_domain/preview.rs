use serde::{Deserialize, Serialize};

use crate::ids::ServiceId;
use crate::profiles::{AgentProvider, ThinkingEffort};

use super::{ButtonLabel, Theme};
use super::{
    ResolvedTriggerProfiles, TriggerAvailability, TriggerContent, TriggerError, TriggerRevision,
};

const MAX_FIRST_MESSAGE_PREVIEW_CHARS: usize = 140;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerTrustPreview {
    pub install_commands_present: bool,
    pub commands_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerSizePreview {
    pub cpu_millis: u32,
    pub memory_mib: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerTechnicalPreview {
    pub provider: AgentProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub effort: ThinkingEffort,
    pub size: TriggerSizePreview,
    pub services: Vec<ServiceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerPreview {
    pub title: String,
    pub description: String,
    pub button_label: ButtonLabel,
    pub theme: Theme,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signup_hint: Option<String>,
    pub publisher_display_name: String,
    pub what_happens: Vec<String>,
    pub availability: TriggerAvailability,
    pub trust: TriggerTrustPreview,
    pub technical: TriggerTechnicalPreview,
    pub first_message: String,
}

impl TriggerPreview {
    pub fn project(
        availability: TriggerAvailability,
        revision: &TriggerRevision,
        publisher_display_name: String,
        profiles: &ResolvedTriggerProfiles,
    ) -> Result<Self, TriggerError> {
        if availability == TriggerAvailability::Draft {
            return Err(TriggerError::new("trigger_not_published", "availability"));
        }
        if publisher_display_name.trim().is_empty() {
            return Err(TriggerError::new(
                "invalid_publisher_display_name",
                "publisher_display_name",
            ));
        }
        revision.validate()?;
        require_matching_profiles(&revision.content, profiles)?;
        let content = &revision.content;
        Ok(Self {
            title: content.presentation.title.clone(),
            description: content.presentation.description.clone(),
            button_label: content.presentation.button_label,
            theme: content.presentation.theme,
            signup_hint: content.presentation.signup_hint.clone(),
            publisher_display_name,
            what_happens: content.reuse.what_happens(),
            availability,
            trust: TriggerTrustPreview {
                install_commands_present: !profiles
                    .sandbox
                    .content
                    .software_install_commands
                    .is_empty(),
                commands_digest: profiles.sandbox.digest.clone(),
            },
            technical: TriggerTechnicalPreview {
                provider: profiles.sandbox.content.agent_provider,
                model: profiles.agent.content.model.clone(),
                effort: profiles.agent.content.thinking_effort,
                size: TriggerSizePreview {
                    cpu_millis: profiles.sandbox.content.cpu_millis,
                    memory_mib: profiles.sandbox.content.memory_mib,
                },
                services: profiles.sandbox.content.nyxid_services.clone(),
            },
            first_message: content
                .first_message
                .chars()
                .take(MAX_FIRST_MESSAGE_PREVIEW_CHARS)
                .collect(),
        })
    }
}

fn require_matching_profiles(
    content: &TriggerContent,
    profiles: &ResolvedTriggerProfiles,
) -> Result<(), TriggerError> {
    let target = &content.target;
    if profiles.sandbox.profile_id != target.sandbox_profile.id
        || profiles.sandbox.revision != target.sandbox_profile.revision
    {
        return Err(TriggerError::new(
            "trigger_reference_mismatch",
            "target.sandbox_profile",
        ));
    }
    if profiles.workspace.profile_id != target.workspace_profile.id
        || profiles.workspace.revision != target.workspace_profile.revision
    {
        return Err(TriggerError::new(
            "trigger_reference_mismatch",
            "target.workspace_profile",
        ));
    }
    if profiles.agent.profile_id != target.agent_profile.id
        || profiles.agent.revision != target.agent_profile.revision
    {
        return Err(TriggerError::new(
            "trigger_reference_mismatch",
            "target.agent_profile",
        ));
    }
    Ok(())
}
