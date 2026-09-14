use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use layer2_core::trigger_domain::{
    CreateOrResumeLaunch, CreateServiceConnection, LaunchPageRequest, PlacementId, StartLaunch,
    TriggerId,
};

use crate::rest::{LaunchStateValue, RepeatChoiceValue};

use super::TriggerContentBody;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTriggerRequest {
    pub content: TriggerContentBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTriggerRequest {
    pub draft_version: u64,
    pub content: TriggerContentBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishTriggerRequest {
    pub draft_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePlacementRequest {
    pub source_url: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct TriggerPageQuery {
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct PlacementQuery {
    #[serde(default)]
    pub include_retired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateLaunchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referrer_origin: Option<String>,
    pub start_if_ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<RepeatChoiceValue>,
}

impl CreateLaunchRequest {
    pub fn into_domain(self, idempotency_key: String) -> CreateOrResumeLaunch {
        CreateOrResumeLaunch {
            placement_id: self.placement_id.map(PlacementId),
            referrer_origin: self.referrer_origin,
            start_if_ready: self.start_if_ready,
            choice: self.choice.map(Into::into),
            idempotency_key,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct LaunchPageQuery {
    pub trigger: Option<String>,
    pub state: Option<LaunchStateValue>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

impl LaunchPageQuery {
    pub fn into_domain(self) -> LaunchPageRequest {
        LaunchPageRequest {
            limit: self
                .limit
                .unwrap_or(layer2_core::trigger_domain::DEFAULT_LAUNCH_PAGE_SIZE),
            cursor: self.cursor,
            trigger: self.trigger.map(TriggerId),
            state: self.state.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct LaunchEventQuery {
    pub after: Option<u64>,
    pub limit: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartLaunchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_confirmed_digest: Option<String>,
    #[serde(default)]
    pub acknowledgements: Vec<String>,
}

impl From<StartLaunchRequest> for StartLaunch {
    fn from(value: StartLaunchRequest) -> Self {
        Self {
            trust_confirmed_digest: value.trust_confirmed_digest,
            acknowledgements: value.acknowledgements,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateServiceConnectionRequest {
    pub service_id: String,
    pub launch_id: String,
    pub intent_id: String,
}

impl From<CreateServiceConnectionRequest> for CreateServiceConnection {
    fn from(value: CreateServiceConnectionRequest) -> Self {
        Self {
            service_id: layer2_core::ServiceId::new(value.service_id),
            launch_id: layer2_core::LaunchId(value.launch_id),
            intent_id: value.intent_id,
        }
    }
}
