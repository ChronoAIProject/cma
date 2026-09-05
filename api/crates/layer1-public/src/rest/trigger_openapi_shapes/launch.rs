//! Owner-safe Launch, requirement, trust, and service-connection projections.

use layer2_core::profiles::EnvironmentVariable;
use layer2_core::trigger_domain as domain;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::rest::{
    FirstMessageStateValue, LaunchBlockedReasonView, LaunchEventView, LaunchFailureView,
    LaunchStateValue, LaunchStepView, ServiceRequirementStateValue,
};

use crate::rest::profiles_openapi_shapes::EnvironmentVariableBody;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", content = "id", rename_all = "snake_case")]
pub enum ResolvedResourceView {
    Pending,
    Reused(String),
    Created(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct LaunchResolutionView {
    pub sandbox: ResolvedResourceView,
    pub workspace: ResolvedResourceView,
    pub agent: ResolvedResourceView,
    pub choice_required: bool,
}

impl From<domain::LaunchResolution> for LaunchResolutionView {
    fn from(value: domain::LaunchResolution) -> Self {
        Self {
            sandbox: resolved_resource(value.sandbox),
            workspace: resolved_resource(value.workspace),
            agent: resolved_resource(value.agent),
            choice_required: value.choice_required,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ServiceRequirementView {
    pub service: String,
    pub state: ServiceRequirementStateValue,
}

impl From<domain::ServiceRequirement> for ServiceRequirementView {
    fn from(value: domain::ServiceRequirement) -> Self {
        Self {
            service: value.service.to_string(),
            state: value.state.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RequirementsSnapshotView {
    pub services: Vec<ServiceRequirementView>,
    pub checked_at: String,
}

impl From<domain::RequirementsSnapshot> for RequirementsSnapshotView {
    fn from(value: domain::RequirementsSnapshot) -> Self {
        Self {
            services: value.services.into_iter().map(Into::into).collect(),
            checked_at: value.checked_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewNeedView {
    Connection { service: String },
    TrustConfirmation { commands_digest: String },
    OwnerRequiresReview,
    RepeatChoice,
}

impl From<domain::ReviewNeed> for ReviewNeedView {
    fn from(value: domain::ReviewNeed) -> Self {
        match value {
            domain::ReviewNeed::Connection { service } => Self::Connection {
                service: service.to_string(),
            },
            domain::ReviewNeed::TrustConfirmation { commands_digest } => {
                Self::TrustConfirmation { commands_digest }
            }
            domain::ReviewNeed::OwnerRequiresReview => Self::OwnerRequiresReview,
            domain::ReviewNeed::RepeatChoice => Self::RepeatChoice,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ReviewRequirementsView {
    pub needs: Vec<ReviewNeedView>,
    pub can_start_now: bool,
}

impl From<domain::ReviewRequirements> for ReviewRequirementsView {
    fn from(value: domain::ReviewRequirements) -> Self {
        Self {
            needs: value.needs.into_iter().map(Into::into).collect(),
            can_start_now: value.can_start_now,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchTrustResourceValue {
    Sandbox,
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct LaunchTrustDisclosureView {
    pub resource: LaunchTrustResourceValue,
    pub name: String,
    pub commands_digest: String,
    pub commands: Vec<String>,
    pub environment_variables: Vec<EnvironmentVariableBody>,
}

impl From<domain::LaunchTrustDisclosure> for LaunchTrustDisclosureView {
    fn from(value: domain::LaunchTrustDisclosure) -> Self {
        Self {
            resource: match value.resource {
                domain::LaunchTrustResource::Sandbox => LaunchTrustResourceValue::Sandbox,
                domain::LaunchTrustResource::Workspace => LaunchTrustResourceValue::Workspace,
            },
            name: value.name,
            commands_digest: value.commands_digest,
            commands: value.commands,
            environment_variables: value
                .environment_variables
                .into_iter()
                .map(environment_variable)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct LaunchTrustManifestView {
    pub version: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_digest: Option<String>,
    pub disclosures: Vec<LaunchTrustDisclosureView>,
}

impl From<domain::LaunchTrustManifest> for LaunchTrustManifestView {
    fn from(value: domain::LaunchTrustManifest) -> Self {
        Self {
            version: value.version,
            confirmation_digest: value.confirmation_digest,
            sandbox_digest: value.sandbox_digest,
            workspace_digest: value.workspace_digest,
            disclosures: value.disclosures.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchConsentSurfaceValue {
    Landing,
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct LaunchConfirmationView {
    pub surface: LaunchConsentSurfaceValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_confirmed_digest: Option<String>,
    pub acknowledgements: Vec<String>,
    pub confirmed_at: String,
}

impl From<domain::LaunchConfirmation> for LaunchConfirmationView {
    fn from(value: domain::LaunchConfirmation) -> Self {
        Self {
            surface: match value.surface {
                domain::LaunchConsentSurface::Landing => LaunchConsentSurfaceValue::Landing,
                domain::LaunchConsentSurface::Review => LaunchConsentSurfaceValue::Review,
            },
            trust_confirmed_digest: value.trust_confirmed_digest,
            acknowledgements: value.acknowledgements,
            confirmed_at: value.confirmed_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct LaunchView {
    pub id: String,
    pub trigger_id: String,
    pub revision_digest: String,
    pub public_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referrer_origin: Option<String>,
    pub state: LaunchStateValue,
    pub steps: Vec<LaunchStepView>,
    pub resolution: LaunchResolutionView,
    pub requirements: RequirementsSnapshotView,
    pub review: ReviewRequirementsView,
    pub trust: LaunchTrustManifestView,
    pub first_message: FirstMessageStateValue,
    pub confirmations: Vec<LaunchConfirmationView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<LaunchBlockedReasonView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<LaunchFailureView>,
    pub event_head: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<String>,
    pub revision: u64,
}

impl From<domain::Launch> for LaunchView {
    fn from(value: domain::Launch) -> Self {
        Self {
            id: value.id.to_string(),
            trigger_id: value.trigger_id.to_string(),
            revision_digest: value.revision_digest,
            public_id: value.public_id.to_string(),
            placement_id: value.placement_id.map(|id| id.to_string()),
            referrer_origin: value.referrer_origin,
            state: value.state.into(),
            steps: value.steps.into_iter().map(Into::into).collect(),
            resolution: value.resolution.into(),
            requirements: value.requirements.into(),
            review: value.review.into(),
            trust: value.trust.into(),
            first_message: value.first_message.into(),
            confirmations: value.confirmations.into_iter().map(Into::into).collect(),
            blocked_reason: value.blocked_reason.map(Into::into),
            failure: value.failure.map(Into::into),
            event_head: value.event_head,
            created_at: value.created_at,
            updated_at: value.updated_at,
            started_at: value.started_at,
            ready_at: value.ready_at,
            revision: value.revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct LaunchPageView {
    pub items: Vec<LaunchView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct LaunchEventPageView {
    pub launch_id: String,
    pub events: Vec<LaunchEventView>,
    pub next_cursor: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServiceConnectionStateValue {
    Pending,
    Satisfied,
    Expired,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CreatedServiceConnectionView {
    pub handoff_id: String,
    pub hosted_url: String,
    pub expires_at: String,
}

impl From<domain::ServiceConnectionHandoff> for CreatedServiceConnectionView {
    fn from(value: domain::ServiceConnectionHandoff) -> Self {
        Self {
            handoff_id: value.id.to_string(),
            hosted_url: value.hosted_url,
            expires_at: value.expires_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ServiceConnectionStatusView {
    pub handoff_id: String,
    pub launch_id: String,
    pub service_id: String,
    pub state: ServiceConnectionStateValue,
    pub expires_at: String,
    pub updated_at: String,
}

impl From<domain::ServiceConnectionHandoff> for ServiceConnectionStatusView {
    fn from(value: domain::ServiceConnectionHandoff) -> Self {
        Self {
            handoff_id: value.id.to_string(),
            launch_id: value.launch_id.to_string(),
            service_id: value.service_id.to_string(),
            state: match value.state {
                domain::BrokerHandoffState::Pending => ServiceConnectionStateValue::Pending,
                domain::BrokerHandoffState::Satisfied => ServiceConnectionStateValue::Satisfied,
                domain::BrokerHandoffState::Expired => ServiceConnectionStateValue::Expired,
                domain::BrokerHandoffState::Cancelled => ServiceConnectionStateValue::Cancelled,
                domain::BrokerHandoffState::Failed => ServiceConnectionStateValue::Failed,
            },
            expires_at: value.expires_at,
            updated_at: value.updated_at,
        }
    }
}

fn resolved_resource<T: ToString>(value: domain::ResolvedResource<T>) -> ResolvedResourceView {
    match value {
        domain::ResolvedResource::Pending => ResolvedResourceView::Pending,
        domain::ResolvedResource::Reused(id) => ResolvedResourceView::Reused(id.to_string()),
        domain::ResolvedResource::Created(id) => ResolvedResourceView::Created(id.to_string()),
    }
}

fn environment_variable(value: EnvironmentVariable) -> EnvironmentVariableBody {
    EnvironmentVariableBody {
        name: value.name,
        value: value.value,
    }
}
