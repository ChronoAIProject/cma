use serde::{Deserialize, Serialize};

use crate::ids::ServiceId;
use crate::profiles::{
    content_digest, AgentProvider, EnvironmentVariable, ThinkingEffort, WorkspaceProfileRevision,
};

use super::{ButtonLabel, Theme};
use super::{
    ResolvedTriggerProfiles, TriggerAvailability, TriggerContent, TriggerError, TriggerRevision,
};

const MAX_FIRST_MESSAGE_PREVIEW_CHARS: usize = 140;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerTrustPreview {
    #[serde(default)]
    pub disclosures: Vec<TriggerTrustPreviewDisclosure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resource", rename_all = "snake_case", deny_unknown_fields)]
pub enum TriggerTrustPreviewDisclosure {
    Sandbox {
        commands_digest: String,
        #[serde(default)]
        commands: Vec<String>,
        #[serde(default)]
        environment_variables: Vec<EnvironmentVariable>,
    },
    Workspace {
        name: String,
        commands_digest: String,
        #[serde(default)]
        commands: Vec<String>,
    },
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
    pub always_review: bool,
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
            always_review: content.consent.always_review,
            signup_hint: content.presentation.signup_hint.clone(),
            publisher_display_name,
            what_happens: content.reuse.what_happens(),
            availability,
            trust: project_trust(profiles)?,
            technical: TriggerTechnicalPreview {
                provider: profiles.sandbox.content.agent_provider,
                model: profiles.agent.content.model.clone(),
                effort: profiles.agent.content.thinking_effort,
                size: TriggerSizePreview {
                    cpu_millis: profiles.sandbox.content.cpu_millis,
                    memory_mib: profiles.sandbox.content.memory_mib,
                },
                services: profiles.sandbox.content.connection_services(),
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
        || !digest_matches(&profiles.sandbox.content, &profiles.sandbox.digest)
    {
        return Err(TriggerError::new(
            "trigger_reference_mismatch",
            "target.sandbox_profile",
        ));
    }
    if profiles.workspace.profile_id != target.workspace_profile.id
        || profiles.workspace.revision != target.workspace_profile.revision
        || !digest_matches(&profiles.workspace.content, &profiles.workspace.digest)
    {
        return Err(TriggerError::new(
            "trigger_reference_mismatch",
            "target.workspace_profile",
        ));
    }
    if profiles.agent.profile_id != target.agent_profile.id
        || profiles.agent.revision != target.agent_profile.revision
        || !digest_matches(&profiles.agent.content, &profiles.agent.digest)
    {
        return Err(TriggerError::new(
            "trigger_reference_mismatch",
            "target.agent_profile",
        ));
    }
    if profiles.sandbox.content.workspace_profiles.len() != profiles.sandbox_workspaces.len()
        || profiles
            .sandbox
            .content
            .workspace_profiles
            .iter()
            .zip(&profiles.sandbox_workspaces)
            .any(|(reference, revision)| {
                revision.profile_id != reference.id
                    || revision.revision != reference.revision
                    || !digest_matches(&revision.content, &revision.digest)
            })
    {
        return Err(TriggerError::new(
            "trigger_reference_mismatch",
            "target.sandbox_profile.workspace_profiles",
        ));
    }
    Ok(())
}

fn project_trust(profiles: &ResolvedTriggerProfiles) -> Result<TriggerTrustPreview, TriggerError> {
    let mut disclosures = Vec::new();
    let sandbox = &profiles.sandbox;
    if !sandbox.content.software_install_commands.is_empty()
        || !sandbox.content.environment_variables.is_empty()
    {
        disclosures.push(TriggerTrustPreviewDisclosure::Sandbox {
            commands_digest: sandbox.digest.clone(),
            commands: sandbox.content.software_install_commands.clone(),
            environment_variables: sandbox.content.environment_variables.clone(),
        });
    }
    for workspace in &profiles.sandbox_workspaces {
        push_workspace_disclosure(
            &mut disclosures,
            workspace,
            "target.sandbox_profile.workspace_profiles",
        )?;
    }
    let target_is_nested = sandbox
        .content
        .workspace_profiles
        .iter()
        .any(|reference| reference.id == profiles.workspace.profile_id);
    if !target_is_nested {
        push_workspace_disclosure(
            &mut disclosures,
            &profiles.workspace,
            "target.workspace_profile",
        )?;
    }
    Ok(TriggerTrustPreview { disclosures })
}

fn push_workspace_disclosure(
    disclosures: &mut Vec<TriggerTrustPreviewDisclosure>,
    revision: &WorkspaceProfileRevision,
    field: &'static str,
) -> Result<(), TriggerError> {
    if revision.content.setup_commands.is_empty() {
        return Ok(());
    }
    let name = revision
        .content
        .workspace_name()
        .map_err(|_| TriggerError::new("trigger_reference_mismatch", field))?;
    disclosures.push(TriggerTrustPreviewDisclosure::Workspace {
        name,
        commands_digest: revision.digest.clone(),
        commands: revision.content.setup_commands.clone(),
    });
    Ok(())
}

fn digest_matches<T: Serialize>(content: &T, digest: &str) -> bool {
    content_digest(content).is_ok_and(|expected| expected == digest)
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
