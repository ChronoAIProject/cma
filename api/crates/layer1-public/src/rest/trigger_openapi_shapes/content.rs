use layer2_core::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};
use layer2_core::trigger_domain as domain;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::rest::profiles_openapi_shapes::{
    AgentProfileReference, PermissionLevelValue, SandboxProfileReference, ThinkingEffortValue,
    WorkspaceProfileReference,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TriggerButtonLabelValue {
    OpenInCma,
    ChatWithThis,
    RunWithCma,
    TryIt,
}

impl From<TriggerButtonLabelValue> for domain::ButtonLabel {
    fn from(value: TriggerButtonLabelValue) -> Self {
        match value {
            TriggerButtonLabelValue::OpenInCma => Self::OpenInCma,
            TriggerButtonLabelValue::ChatWithThis => Self::ChatWithThis,
            TriggerButtonLabelValue::RunWithCma => Self::RunWithCma,
            TriggerButtonLabelValue::TryIt => Self::TryIt,
        }
    }
}

impl From<domain::ButtonLabel> for TriggerButtonLabelValue {
    fn from(value: domain::ButtonLabel) -> Self {
        match value {
            domain::ButtonLabel::OpenInCma => Self::OpenInCma,
            domain::ButtonLabel::ChatWithThis => Self::ChatWithThis,
            domain::ButtonLabel::RunWithCma => Self::RunWithCma,
            domain::ButtonLabel::TryIt => Self::TryIt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TriggerThemeValue {
    Auto,
    Light,
    Dark,
}

impl From<TriggerThemeValue> for domain::Theme {
    fn from(value: TriggerThemeValue) -> Self {
        match value {
            TriggerThemeValue::Auto => Self::Auto,
            TriggerThemeValue::Light => Self::Light,
            TriggerThemeValue::Dark => Self::Dark,
        }
    }
}

impl From<domain::Theme> for TriggerThemeValue {
    fn from(value: domain::Theme) -> Self {
        match value {
            domain::Theme::Auto => Self::Auto,
            domain::Theme::Light => Self::Light,
            domain::Theme::Dark => Self::Dark,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SandboxReusePolicyValue {
    ReuseExisting,
    AlwaysNew,
}

impl From<SandboxReusePolicyValue> for domain::SandboxReusePolicy {
    fn from(value: SandboxReusePolicyValue) -> Self {
        match value {
            SandboxReusePolicyValue::ReuseExisting => Self::ReuseExisting,
            SandboxReusePolicyValue::AlwaysNew => Self::AlwaysNew,
        }
    }
}

impl From<domain::SandboxReusePolicy> for SandboxReusePolicyValue {
    fn from(value: domain::SandboxReusePolicy) -> Self {
        match value {
            domain::SandboxReusePolicy::ReuseExisting => Self::ReuseExisting,
            domain::SandboxReusePolicy::AlwaysNew => Self::AlwaysNew,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceReusePolicyValue {
    ReuseExisting,
    AlwaysNew,
}

impl From<WorkspaceReusePolicyValue> for domain::WorkspaceReusePolicy {
    fn from(value: WorkspaceReusePolicyValue) -> Self {
        match value {
            WorkspaceReusePolicyValue::ReuseExisting => Self::ReuseExisting,
            WorkspaceReusePolicyValue::AlwaysNew => Self::AlwaysNew,
        }
    }
}

impl From<domain::WorkspaceReusePolicy> for WorkspaceReusePolicyValue {
    fn from(value: domain::WorkspaceReusePolicy) -> Self {
        match value {
            domain::WorkspaceReusePolicy::ReuseExisting => Self::ReuseExisting,
            domain::WorkspaceReusePolicy::AlwaysNew => Self::AlwaysNew,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentReusePolicyValue {
    ResumeExisting,
    AlwaysNew,
    Ask,
}

impl From<AgentReusePolicyValue> for domain::AgentReusePolicy {
    fn from(value: AgentReusePolicyValue) -> Self {
        match value {
            AgentReusePolicyValue::ResumeExisting => Self::ResumeExisting,
            AgentReusePolicyValue::AlwaysNew => Self::AlwaysNew,
            AgentReusePolicyValue::Ask => Self::Ask,
        }
    }
}

impl From<domain::AgentReusePolicy> for AgentReusePolicyValue {
    fn from(value: domain::AgentReusePolicy) -> Self {
        match value {
            domain::AgentReusePolicy::ResumeExisting => Self::ResumeExisting,
            domain::AgentReusePolicy::AlwaysNew => Self::AlwaysNew,
            domain::AgentReusePolicy::Ask => Self::Ask,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerPresentationBody {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub button_label: TriggerButtonLabelValue,
    pub theme: TriggerThemeValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signup_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerTargetBody {
    #[serde(default)]
    pub sandbox_profile: SandboxProfileReference,
    #[serde(default)]
    pub workspace_profile: WorkspaceProfileReference,
    #[serde(default)]
    pub agent_profile: AgentProfileReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<TriggerInlineTargetBody>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerImageBody {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerRepositoryBody {
    pub url: String,
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerInlineWorkspaceBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<TriggerRepositoryBody>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerInlineAgentBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub reasoning_effort: ThinkingEffortValue,
    pub permission_mode: PermissionLevelValue,
    #[serde(default)]
    pub skills: Vec<TriggerSkillBody>,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub services: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerSkillBody {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerInlineTargetBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<TriggerImageBody>,
    pub agent: TriggerInlineAgentBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<TriggerInlineWorkspaceBody>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerReuseBody {
    pub sandbox: SandboxReusePolicyValue,
    pub workspace: WorkspaceReusePolicyValue,
    pub agent: AgentReusePolicyValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerConsentBody {
    pub always_review: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerContentBody {
    pub presentation: TriggerPresentationBody,
    pub target: TriggerTargetBody,
    pub reuse: TriggerReuseBody,
    #[serde(default)]
    pub first_message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quick_messages: Vec<TriggerQuickMessageBody>,
    pub consent: TriggerConsentBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerQuickMessageBody {
    pub name: String,
    pub description: String,
    pub text: String,
}

impl From<TriggerQuickMessageBody> for domain::TriggerQuickMessage {
    fn from(value: TriggerQuickMessageBody) -> Self {
        Self {
            name: value.name,
            description: value.description,
            text: value.text,
        }
    }
}

impl From<domain::TriggerQuickMessage> for TriggerQuickMessageBody {
    fn from(value: domain::TriggerQuickMessage) -> Self {
        Self {
            name: value.name,
            description: value.description,
            text: value.text,
        }
    }
}

impl From<TriggerContentBody> for domain::TriggerContent {
    fn from(value: TriggerContentBody) -> Self {
        Self {
            presentation: domain::Presentation {
                title: value.presentation.title,
                description: value.presentation.description,
                button_label: value.presentation.button_label.into(),
                theme: value.presentation.theme.into(),
                signup_hint: value.presentation.signup_hint,
            },
            target: domain::TriggerTarget {
                sandbox_profile: SandboxProfileRef::new(
                    layer2_core::SandboxProfileId(value.target.sandbox_profile.id),
                    value.target.sandbox_profile.revision,
                ),
                workspace_profile: WorkspaceProfileRef::new(
                    layer2_core::WorkspaceProfileId(value.target.workspace_profile.id),
                    value.target.workspace_profile.revision,
                ),
                agent_profile: AgentProfileRef::new(
                    layer2_core::AgentProfileId(value.target.agent_profile.id),
                    value.target.agent_profile.revision,
                ),
                inline: value
                    .target
                    .inline
                    .map(|inline| domain::TriggerInlineTarget {
                        image: inline.image.map(|image| domain::TriggerImage {
                            id: image.id,
                            version: image.version,
                            reference: image.reference,
                        }),
                        agent: domain::TriggerInlineAgent {
                            model: inline.agent.model,
                            reasoning_effort: inline.agent.reasoning_effort.into(),
                            permission_mode: inline.agent.permission_mode.into(),
                            skills: inline
                                .agent
                                .skills
                                .into_iter()
                                .map(|skill| layer2_core::profiles::SkillRef {
                                    source: layer2_core::profiles::SkillSource::Ornn,
                                    id: layer2_core::SkillId::new(skill.id),
                                    version: skill.version,
                                })
                                .collect(),
                            system_prompt: inline.agent.system_prompt,
                            services: inline
                                .agent
                                .services
                                .into_iter()
                                .map(layer2_core::ServiceId::new)
                                .collect(),
                        },
                        workspace: inline.workspace.map(|workspace| {
                            domain::TriggerInlineWorkspace {
                                repository: workspace.repository.map(|repository| {
                                    domain::TriggerRepository {
                                        url: repository.url,
                                        reference: repository.reference,
                                    }
                                }),
                            }
                        }),
                    }),
            },
            reuse: domain::ReusePolicy {
                sandbox: value.reuse.sandbox.into(),
                workspace: value.reuse.workspace.into(),
                agent: value.reuse.agent.into(),
            },
            first_message: value.first_message,
            quick_messages: value.quick_messages.into_iter().map(Into::into).collect(),
            consent: domain::TriggerConsent {
                always_review: value.consent.always_review,
            },
        }
    }
}

impl From<domain::TriggerContent> for TriggerContentBody {
    fn from(value: domain::TriggerContent) -> Self {
        Self {
            presentation: TriggerPresentationBody {
                title: value.presentation.title,
                description: value.presentation.description,
                button_label: value.presentation.button_label.into(),
                theme: value.presentation.theme.into(),
                signup_hint: value.presentation.signup_hint,
            },
            target: TriggerTargetBody {
                sandbox_profile: SandboxProfileReference {
                    id: value.target.sandbox_profile.id.to_string(),
                    revision: value.target.sandbox_profile.revision,
                },
                workspace_profile: WorkspaceProfileReference {
                    id: value.target.workspace_profile.id.to_string(),
                    revision: value.target.workspace_profile.revision,
                },
                agent_profile: AgentProfileReference {
                    id: value.target.agent_profile.id.to_string(),
                    revision: value.target.agent_profile.revision,
                },
                inline: value.target.inline.map(|inline| TriggerInlineTargetBody {
                    image: inline.image.map(|image| TriggerImageBody {
                        id: image.id,
                        version: image.version,
                        reference: image.reference,
                    }),
                    agent: TriggerInlineAgentBody {
                        model: inline.agent.model,
                        reasoning_effort: inline.agent.reasoning_effort.into(),
                        permission_mode: inline.agent.permission_mode.into(),
                        skills: inline
                            .agent
                            .skills
                            .into_iter()
                            .map(|skill| TriggerSkillBody {
                                id: skill.id.to_string(),
                                version: skill.version,
                            })
                            .collect(),
                        system_prompt: inline.agent.system_prompt,
                        services: inline
                            .agent
                            .services
                            .into_iter()
                            .map(|service| service.to_string())
                            .collect(),
                    },
                    workspace: inline
                        .workspace
                        .map(|workspace| TriggerInlineWorkspaceBody {
                            repository: workspace.repository.map(|repository| {
                                TriggerRepositoryBody {
                                    url: repository.url,
                                    reference: repository.reference,
                                }
                            }),
                        }),
                }),
            },
            reuse: TriggerReuseBody {
                sandbox: value.reuse.sandbox.into(),
                workspace: value.reuse.workspace.into(),
                agent: value.reuse.agent.into(),
            },
            first_message: value.first_message,
            quick_messages: value.quick_messages.into_iter().map(Into::into).collect(),
            consent: TriggerConsentBody {
                always_review: value.consent.always_review,
            },
        }
    }
}
