use super::*;
use crate::rest::bots::shapes::ProfileRef;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    pub name: String,
    pub description: String,
    pub agent_profile: ProfileRef,
    pub workspace_profile: ProfileRef,
    pub sandbox_profile: ProfileRef,
}
impl From<ProfileInput> for EventWorkerProfileInput {
    fn from(v: ProfileInput) -> Self {
        Self {
            name: v.name,
            description: v.description,
            agent_profile: layer2_core::profiles::AgentProfileRef::new(
                layer2_core::AgentProfileId(v.agent_profile.id),
                v.agent_profile.revision,
            ),
            workspace_profile: layer2_core::profiles::WorkspaceProfileRef::new(
                layer2_core::WorkspaceProfileId(v.workspace_profile.id),
                v.workspace_profile.revision,
            ),
            sandbox_profile: layer2_core::profiles::SandboxProfileRef::new(
                layer2_core::SandboxProfileId(v.sandbox_profile.id),
                v.sandbox_profile.revision,
            ),
        }
    }
}
impl From<EventWorkerProfileInput> for ProfileInput {
    fn from(v: EventWorkerProfileInput) -> Self {
        Self {
            name: v.name,
            description: v.description,
            agent_profile: ProfileRef {
                id: v.agent_profile.id.to_string(),
                revision: v.agent_profile.revision,
            },
            workspace_profile: ProfileRef {
                id: v.workspace_profile.id.to_string(),
                revision: v.workspace_profile.revision,
            },
            sandbox_profile: ProfileRef {
                id: v.sandbox_profile.id.to_string(),
                revision: v.sandbox_profile.revision,
            },
        }
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    pub expected_revision: u32,
    pub source: ProfileInput,
}
#[derive(Serialize, ToSchema)]
pub struct ProfileView {
    pub id: String,
    pub owner: String,
    pub revision: u32,
    pub source: ProfileInput,
    pub profile: serde_json::Value,
    pub digest: String,
    pub created_at: String,
    pub updated_at: String,
    pub retired_at: Option<String>,
}
impl From<EventWorkerProfile> for ProfileView {
    fn from(v: EventWorkerProfile) -> Self {
        Self {
            id: v.id,
            owner: v.owner.to_string(),
            revision: v.revision,
            source: v.source.into(),
            profile: v.snapshot,
            digest: v.digest,
            created_at: v.created_at,
            updated_at: v.updated_at,
            retired_at: v.retired_at,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub struct ProfilePage {
    pub items: Vec<ProfileView>,
    pub next_after: Option<String>,
}
