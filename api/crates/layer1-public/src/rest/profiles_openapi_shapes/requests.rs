use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::{
    AgentProfileContentBody, ProfileVisibilityValue, SandboxProfileContentBody,
    WorkspaceProfileContentBody,
};

macro_rules! profile_requests {
    ($create:ident, $update:ident, $content:ty) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $create {
            pub name: String,
            #[serde(default)]
            pub description: String,
            pub content: $content,
        }

        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $update {
            pub draft_version: u64,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub name: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub description: Option<String>,
            pub content: $content,
        }
    };
}

profile_requests!(
    CreateAgentProfileRequest,
    UpdateAgentProfileRequest,
    AgentProfileContentBody
);
profile_requests!(
    CreateWorkspaceProfileRequest,
    UpdateWorkspaceProfileRequest,
    WorkspaceProfileContentBody
);
profile_requests!(
    CreateSandboxProfileRequest,
    UpdateSandboxProfileRequest,
    SandboxProfileContentBody
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishProfileRequest {
    pub draft_version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetProfileVisibilityRequest {
    pub visibility: ProfileVisibilityValue,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkProfileRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct ProfilePageQuery {
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct MarketplacePageQuery {
    pub q: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
