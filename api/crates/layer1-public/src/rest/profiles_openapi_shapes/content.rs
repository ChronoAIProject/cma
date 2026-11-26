use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
    AgentProfileReference, AgentProviderValue, ModelAccessValue, PermissionLevelValue,
    ThinkingEffortValue, WorkspaceProfileReference,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillReference {
    pub source: SkillSourceValue,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillSourceValue {
    Ornn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileContentBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub thinking_effort: ThinkingEffortValue,
    pub permission_level: PermissionLevelValue,
    #[serde(default)]
    pub skills: Vec<SkillReference>,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub isolated_agent: bool,
}

impl From<AgentProfileContentBody> for layer2_core::profiles::AgentProfileContent {
    fn from(value: AgentProfileContentBody) -> Self {
        Self {
            model: value.model,
            thinking_effort: value.thinking_effort.into(),
            permission_level: value.permission_level.into(),
            skills: value
                .skills
                .into_iter()
                .map(|skill| layer2_core::profiles::SkillRef {
                    source: layer2_core::profiles::SkillSource::Ornn,
                    id: layer2_core::SkillId::new(skill.id),
                    version: skill.version,
                })
                .collect(),
            system_prompt: value.system_prompt,
            isolated_agent: value.isolated_agent,
        }
    }
}

impl From<layer2_core::profiles::AgentProfileContent> for AgentProfileContentBody {
    fn from(value: layer2_core::profiles::AgentProfileContent) -> Self {
        Self {
            model: value.model,
            thinking_effort: value.thinking_effort.into(),
            permission_level: value.permission_level.into(),
            skills: value
                .skills
                .into_iter()
                .map(|skill| SkillReference {
                    source: SkillSourceValue::Ornn,
                    id: skill.id.to_string(),
                    version: skill.version,
                })
                .collect(),
            system_prompt: value.system_prompt,
            isolated_agent: value.isolated_agent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceLocationBody {
    RepositorySnapshot {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    GitRepository {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    Directory {
        name: String,
    },
}

impl From<WorkspaceLocationBody> for layer2_core::profiles::WorkspaceLocation {
    fn from(value: WorkspaceLocationBody) -> Self {
        match value {
            WorkspaceLocationBody::RepositorySnapshot { url, reference } => {
                Self::RepositorySnapshot {
                    url,
                    reference,
                    connection_id: None,
                }
            }
            WorkspaceLocationBody::GitRepository { url, reference } => {
                Self::GitRepository { url, reference }
            }
            WorkspaceLocationBody::Directory { name } => Self::Directory { name },
        }
    }
}

impl From<layer2_core::profiles::WorkspaceLocation> for WorkspaceLocationBody {
    fn from(value: layer2_core::profiles::WorkspaceLocation) -> Self {
        match value {
            layer2_core::profiles::WorkspaceLocation::RepositorySnapshot {
                url, reference, ..
            } => Self::RepositorySnapshot { url, reference },
            layer2_core::profiles::WorkspaceLocation::PublicRepositorySnapshot {
                url,
                commit,
                ..
            } => Self::RepositorySnapshot {
                url,
                reference: Some(commit),
            },
            layer2_core::profiles::WorkspaceLocation::GitRepository { url, reference } => {
                Self::GitRepository { url, reference }
            }
            layer2_core::profiles::WorkspaceLocation::Directory { name } => {
                Self::Directory { name }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileContentBody {
    pub location: WorkspaceLocationBody,
    pub name: String,
    #[serde(default)]
    pub setup_commands: Vec<String>,
    #[serde(default)]
    pub agent_profiles: Vec<AgentProfileReference>,
}

impl From<WorkspaceProfileContentBody> for layer2_core::profiles::WorkspaceProfileContent {
    fn from(value: WorkspaceProfileContentBody) -> Self {
        Self {
            location: value.location.into(),
            name: value.name,
            setup_commands: value.setup_commands,
            agent_profiles: value
                .agent_profiles
                .into_iter()
                .map(|reference| {
                    layer2_core::profiles::AgentProfileRef::new(
                        layer2_core::AgentProfileId(reference.id),
                        reference.revision,
                    )
                })
                .collect(),
        }
    }
}

impl From<layer2_core::profiles::WorkspaceProfileContent> for WorkspaceProfileContentBody {
    fn from(value: layer2_core::profiles::WorkspaceProfileContent) -> Self {
        Self {
            location: value.location.into(),
            name: value.name,
            setup_commands: value.setup_commands,
            agent_profiles: value
                .agent_profiles
                .into_iter()
                .map(|reference| AgentProfileReference {
                    id: reference.id.to_string(),
                    revision: reference.revision,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentVariableBody {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SandboxProfileContentBody {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    #[serde(default)]
    pub nyxid_services: Vec<String>,
    pub agent_provider: AgentProviderValue,
    pub model_access: ModelAccessValue,
    pub max_workspaces: u8,
    pub max_agents_per_workspace: u8,
    #[serde(default)]
    pub workspace_profiles: Vec<WorkspaceProfileReference>,
    #[serde(default)]
    pub software_install_commands: Vec<String>,
    #[serde(default)]
    pub environment_variables: Vec<EnvironmentVariableBody>,
}

impl From<SandboxProfileContentBody> for layer2_core::profiles::SandboxProfileContent {
    fn from(value: SandboxProfileContentBody) -> Self {
        Self {
            runtime_image: None,
            cpu_millis: value.cpu_millis,
            memory_mib: value.memory_mib,
            nyxid_services: value
                .nyxid_services
                .into_iter()
                .map(layer2_core::ServiceId::new)
                .collect(),
            agent_provider: layer2_core::profiles::AgentProvider::Codex,
            model_access: match value.model_access {
                ModelAccessValue::ChronoaiManaged => {
                    layer2_core::profiles::ModelAccess::ChronoaiManaged
                }
                ModelAccessValue::OwnCredentials => {
                    layer2_core::profiles::ModelAccess::OwnCredentials
                }
            },
            max_workspaces: value.max_workspaces,
            max_agents_per_workspace: value.max_agents_per_workspace,
            workspace_profiles: value
                .workspace_profiles
                .into_iter()
                .map(|reference| {
                    layer2_core::profiles::WorkspaceProfileRef::new(
                        layer2_core::WorkspaceProfileId(reference.id),
                        reference.revision,
                    )
                })
                .collect(),
            software_install_commands: value.software_install_commands,
            environment_variables: value
                .environment_variables
                .into_iter()
                .map(|variable| layer2_core::profiles::EnvironmentVariable {
                    name: variable.name,
                    value: variable.value,
                })
                .collect(),
        }
    }
}

impl From<layer2_core::profiles::SandboxProfileContent> for SandboxProfileContentBody {
    fn from(value: layer2_core::profiles::SandboxProfileContent) -> Self {
        Self {
            cpu_millis: value.cpu_millis,
            memory_mib: value.memory_mib,
            nyxid_services: value
                .nyxid_services
                .into_iter()
                .map(|service| service.to_string())
                .collect(),
            agent_provider: AgentProviderValue::Codex,
            model_access: match value.model_access {
                layer2_core::profiles::ModelAccess::ChronoaiManaged => {
                    ModelAccessValue::ChronoaiManaged
                }
                layer2_core::profiles::ModelAccess::OwnCredentials => {
                    ModelAccessValue::OwnCredentials
                }
            },
            max_workspaces: value.max_workspaces,
            max_agents_per_workspace: value.max_agents_per_workspace,
            workspace_profiles: value
                .workspace_profiles
                .into_iter()
                .map(|reference| WorkspaceProfileReference {
                    id: reference.id.to_string(),
                    revision: reference.revision,
                })
                .collect(),
            software_install_commands: value.software_install_commands,
            environment_variables: value
                .environment_variables
                .into_iter()
                .map(|variable| EnvironmentVariableBody {
                    name: variable.name,
                    value: variable.value,
                })
                .collect(),
        }
    }
}
