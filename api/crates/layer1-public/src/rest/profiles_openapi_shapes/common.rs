use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileVisibilityValue {
    Private,
    Public,
}

impl From<ProfileVisibilityValue> for layer2_core::profiles::ProfileVisibility {
    fn from(value: ProfileVisibilityValue) -> Self {
        match value {
            ProfileVisibilityValue::Private => Self::Private,
            ProfileVisibilityValue::Public => Self::Public,
        }
    }
}

impl From<layer2_core::profiles::ProfileVisibility> for ProfileVisibilityValue {
    fn from(value: layer2_core::profiles::ProfileVisibility) -> Self {
        match value {
            layer2_core::profiles::ProfileVisibility::Private => Self::Private,
            layer2_core::profiles::ProfileVisibility::Public => Self::Public,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingEffortValue {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl From<ThinkingEffortValue> for layer2_core::profiles::ThinkingEffort {
    fn from(value: ThinkingEffortValue) -> Self {
        use layer2_core::profiles::ThinkingEffort as Domain;
        match value {
            ThinkingEffortValue::Minimal => Domain::Minimal,
            ThinkingEffortValue::Low => Domain::Low,
            ThinkingEffortValue::Medium => Domain::Medium,
            ThinkingEffortValue::High => Domain::High,
            ThinkingEffortValue::XHigh => Domain::XHigh,
        }
    }
}

impl From<layer2_core::profiles::ThinkingEffort> for ThinkingEffortValue {
    fn from(value: layer2_core::profiles::ThinkingEffort) -> Self {
        use layer2_core::profiles::ThinkingEffort as Domain;
        match value {
            Domain::Minimal => Self::Minimal,
            Domain::Low => Self::Low,
            Domain::Medium => Self::Medium,
            Domain::High => Self::High,
            Domain::XHigh => Self::XHigh,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevelValue {
    Guarded,
    Autonomous,
}

impl From<PermissionLevelValue> for layer2_core::profiles::PermissionLevel {
    fn from(value: PermissionLevelValue) -> Self {
        match value {
            PermissionLevelValue::Guarded => Self::Guarded,
            PermissionLevelValue::Autonomous => Self::Autonomous,
        }
    }
}

impl From<layer2_core::profiles::PermissionLevel> for PermissionLevelValue {
    fn from(value: layer2_core::profiles::PermissionLevel) -> Self {
        match value {
            layer2_core::profiles::PermissionLevel::Guarded => Self::Guarded,
            layer2_core::profiles::PermissionLevel::Autonomous => Self::Autonomous,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentProviderValue {
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelAccessValue {
    ChronoaiManaged,
    OwnCredentials,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceKindValue {
    Agent,
    Workspace,
    Sandbox,
}

impl From<layer2_core::profiles::MarketplaceKind> for MarketplaceKindValue {
    fn from(value: layer2_core::profiles::MarketplaceKind) -> Self {
        match value {
            layer2_core::profiles::MarketplaceKind::Agent => Self::Agent,
            layer2_core::profiles::MarketplaceKind::Workspace => Self::Workspace,
            layer2_core::profiles::MarketplaceKind::Sandbox => Self::Sandbox,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileReference {
    pub id: String,
    pub revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileReference {
    pub id: String,
    pub revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct SandboxProfileReference {
    pub id: String,
    pub revision: u32,
}
