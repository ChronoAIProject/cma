use layer2_core::profiles::{AgentProvider, EnvironmentVariable, ThinkingEffort};
use layer2_core::trigger_domain as domain;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::rest::profiles_openapi_shapes::{
    AgentProviderValue, EnvironmentVariableBody, ThinkingEffortValue,
};

use super::{TriggerButtonLabelValue, TriggerContentBody, TriggerThemeValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TriggerAvailabilityValue {
    Draft,
    Published,
    Retired,
}

impl From<domain::TriggerAvailability> for TriggerAvailabilityValue {
    fn from(value: domain::TriggerAvailability) -> Self {
        match value {
            domain::TriggerAvailability::Draft => Self::Draft,
            domain::TriggerAvailability::Published => Self::Published,
            domain::TriggerAvailability::Retired => Self::Retired,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TriggerStatsView {
    pub launches_total: u64,
    pub agents_ready_total: u64,
}

impl From<domain::TriggerStats> for TriggerStatsView {
    fn from(value: domain::TriggerStats) -> Self {
        Self {
            launches_total: value.launches_total,
            agents_ready_total: value.agents_ready_total,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PlacementView {
    pub id: String,
    pub source_url: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl From<domain::Placement> for PlacementView {
    fn from(value: domain::Placement) -> Self {
        Self {
            id: value.id.to_string(),
            source_url: value.source_url,
            label: value.label,
            retired_at: value.retired_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TriggerRevisionView {
    pub revision: u32,
    pub content: TriggerContentBody,
    pub digest: String,
    pub published_at: String,
}

impl From<domain::TriggerRevision> for TriggerRevisionView {
    fn from(value: domain::TriggerRevision) -> Self {
        Self {
            revision: value.revision,
            content: value.content.into(),
            digest: value.digest,
            published_at: value.published_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PublishedTriggerRevisionView {
    pub trigger_id: String,
    pub public_id: String,
    pub availability: TriggerAvailabilityValue,
    #[serde(flatten)]
    pub revision: TriggerRevisionView,
}

impl From<domain::PublicTrigger> for PublishedTriggerRevisionView {
    fn from(value: domain::PublicTrigger) -> Self {
        Self {
            trigger_id: value.trigger_id.to_string(),
            public_id: value.public_id.to_string(),
            availability: value.availability.into(),
            revision: value.revision.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TriggerView {
    pub id: String,
    pub draft: TriggerContentBody,
    pub draft_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_id: Option<String>,
    pub availability: TriggerAvailabilityValue,
    pub revision_count: u32,
    pub placements: Vec<PlacementView>,
    pub stats: TriggerStatsView,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl From<domain::Trigger> for TriggerView {
    fn from(value: domain::Trigger) -> Self {
        Self {
            id: value.id.to_string(),
            draft: value.draft.into(),
            draft_version: value.draft_version,
            public_id: value.public_id.map(|id| id.to_string()),
            availability: value.availability.into(),
            revision_count: u32::try_from(value.revisions.len()).unwrap_or(u32::MAX),
            placements: value.placements.into_iter().map(Into::into).collect(),
            stats: value.stats.into(),
            created_at: value.created_at,
            updated_at: value.updated_at,
            retired_at: value.retired_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TriggerPageView {
    pub items: Vec<TriggerView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TriggerPlacementStatsView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement_id: Option<String>,
    pub launches_total: u64,
    pub agents_ready_total: u64,
}

impl From<domain::LaunchPlacementStats> for TriggerPlacementStatsView {
    fn from(value: domain::LaunchPlacementStats) -> Self {
        Self {
            placement_id: value.placement_id.map(|id| id.to_string()),
            launches_total: value.launches_total,
            agents_ready_total: value.agents_ready_total,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TriggerStatisticsView {
    pub totals: TriggerStatsView,
    pub placements: Vec<TriggerPlacementStatsView>,
}

impl From<domain::TriggerStatistics> for TriggerStatisticsView {
    fn from(value: domain::TriggerStatistics) -> Self {
        Self {
            totals: value.totals.into(),
            placements: value.placements.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "resource", rename_all = "snake_case")]
pub enum TriggerTrustDisclosureView {
    Sandbox {
        commands_digest: String,
        commands: Vec<String>,
        environment_variables: Vec<EnvironmentVariableBody>,
    },
    Workspace {
        name: String,
        commands_digest: String,
        commands: Vec<String>,
    },
}

impl From<domain::TriggerTrustPreviewDisclosure> for TriggerTrustDisclosureView {
    fn from(value: domain::TriggerTrustPreviewDisclosure) -> Self {
        match value {
            domain::TriggerTrustPreviewDisclosure::Sandbox {
                commands_digest,
                commands,
                environment_variables,
            } => Self::Sandbox {
                commands_digest,
                commands,
                environment_variables: environment_variables
                    .into_iter()
                    .map(environment_variable)
                    .collect(),
            },
            domain::TriggerTrustPreviewDisclosure::Workspace {
                name,
                commands_digest,
                commands,
            } => Self::Workspace {
                name,
                commands_digest,
                commands,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TriggerTrustPreviewView {
    pub disclosures: Vec<TriggerTrustDisclosureView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct TriggerSizePreviewView {
    pub cpu_millis: u32,
    pub memory_mib: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TriggerTechnicalPreviewView {
    pub provider: AgentProviderValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub effort: ThinkingEffortValue,
    pub size: TriggerSizePreviewView,
    pub services: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TriggerPreviewView {
    pub title: String,
    pub description: String,
    pub button_label: TriggerButtonLabelValue,
    pub theme: TriggerThemeValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signup_hint: Option<String>,
    pub publisher_display_name: String,
    pub what_happens: Vec<String>,
    pub availability: TriggerAvailabilityValue,
    pub trust: TriggerTrustPreviewView,
    pub technical: TriggerTechnicalPreviewView,
    pub first_message: String,
}

impl From<domain::TriggerPreview> for TriggerPreviewView {
    fn from(value: domain::TriggerPreview) -> Self {
        Self {
            title: value.title,
            description: value.description,
            button_label: value.button_label.into(),
            theme: value.theme.into(),
            signup_hint: value.signup_hint,
            publisher_display_name: value.publisher_display_name,
            what_happens: value.what_happens,
            availability: value.availability.into(),
            trust: TriggerTrustPreviewView {
                disclosures: value
                    .trust
                    .disclosures
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            },
            technical: TriggerTechnicalPreviewView {
                provider: agent_provider(value.technical.provider),
                model: value.technical.model,
                effort: thinking_effort(value.technical.effort),
                size: TriggerSizePreviewView {
                    cpu_millis: value.technical.size.cpu_millis,
                    memory_mib: value.technical.size.memory_mib,
                },
                services: value
                    .technical
                    .services
                    .into_iter()
                    .map(|service| service.to_string())
                    .collect(),
            },
            first_message: value.first_message,
        }
    }
}

fn environment_variable(value: EnvironmentVariable) -> EnvironmentVariableBody {
    EnvironmentVariableBody {
        name: value.name,
        value: value.value,
    }
}

fn agent_provider(value: AgentProvider) -> AgentProviderValue {
    match value {
        AgentProvider::Codex => AgentProviderValue::Codex,
    }
}

fn thinking_effort(value: ThinkingEffort) -> ThinkingEffortValue {
    match value {
        ThinkingEffort::Minimal => ThinkingEffortValue::Minimal,
        ThinkingEffort::Low => ThinkingEffortValue::Low,
        ThinkingEffort::Medium => ThinkingEffortValue::Medium,
        ThinkingEffort::High => ThinkingEffortValue::High,
        ThinkingEffort::XHigh => ThinkingEffortValue::XHigh,
    }
}
