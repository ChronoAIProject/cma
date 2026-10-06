use super::{SetupActionReceipt, SetupReviewEvent};
use crate::{
    profiles::{
        AgentProfileContent, SandboxProfileAvailability, SandboxProfileContent,
        WorkspaceProfileContent,
    },
    resource_access::ResourceOwner,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalActor {
    pub binding_id: String,
    pub channel_id: String,
    pub conversation_id: String,
    pub actor_id: String,
    pub platform: String,
}
impl ExternalActor {
    pub fn key(&self) -> String {
        crate::profiles::content_digest(self).expect("actor strings serialize")
    }
}

#[derive(Debug, Clone)]
pub struct SetupEvent {
    pub actor: ExternalActor,
    pub event_id: String,
    pub reference: String,
    pub occurred_at: String,
    pub input: SetupInput,
}

#[derive(Debug, Clone)]
pub enum SetupInput {
    Text(String),
    /// An authenticated provider notification conveys state, never human consent.
    Notification,
    Interaction {
        block_id: String,
        action_id: String,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupDraft {
    pub name: Option<String>,
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupCustomization {
    pub agent: Option<AgentProfileContent>,
    pub workspace: Option<WorkspaceProfileContent>,
    pub sandbox: Option<SandboxProfileContent>,
    pub availability: Option<SandboxProfileAvailability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupConfiguration {
    pub agent: AgentProfileContent,
    pub workspace: WorkspaceProfileContent,
    pub sandbox: SandboxProfileContent,
    pub availability: SandboxProfileAvailability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStep {
    Login,
    Owner,
    Name,
    Agent,
    AgentCustom,
    Workspace,
    WorkspaceCustom,
    Sandbox,
    SandboxCustom,
    Services,
    ExternalBot,
    Admission,
    Review,
    Provisioning,
    Complete,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotSetup {
    pub id: String,
    pub actor: ExternalActor,
    pub revision: u64,
    pub step: SetupStep,
    pub grant_id: Option<String>,
    pub owner: Option<ResourceOwner>,
    pub draft: SetupDraft,
    pub configuration: SetupConfiguration,
    pub selected_services: Vec<String>,
    pub external_bot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_connection: Option<SetupConnectionRequest>,
    pub admission: Option<crate::bots::BotAdmission>,
    pub service_page: usize,
    pub confirmation_digest: Option<String>,
    #[serde(default)]
    pub confirmation_event: Option<SetupReviewEvent>,
    #[serde(default)]
    pub processed_actions: Vec<SetupActionReceipt>,
    pub bot_id: Option<String>,
    pub processed_events: Vec<String>,
    pub updated_at: String,
}
impl BotSetup {
    pub fn block_id(&self) -> String {
        format!("{}.{}", self.id, self.revision)
    }
    pub fn plan_digest(&self) -> Result<String, crate::bots::BotError> {
        let mut plan = serde_json::json!({
            "id":self.id,"grant":self.grant_id,"owner":self.owner,"name":self.draft.name,
            "configuration":self.configuration,"services":self.selected_services,
            "external_bot_id":self.external_bot_id,"admission":self.admission,
        });
        if let Some(reference) = &self.connection_ref {
            plan["connection_ref"] = reference.clone().into();
        }
        crate::profiles::content_digest(&plan).map_err(|_| crate::bots::BotError::Unavailable)
    }
}

/// Public workflow data. Clients choose their presentation and interaction transport.
#[derive(Debug, Clone, Serialize)]
pub struct SetupView {
    pub setup_id: String,
    pub revision: u64,
    pub step: SetupStep,
    pub interaction_id: String,
    pub identity: SetupIdentity,
    pub owner: Option<ResourceOwner>,
    pub name: Option<String>,
    pub configuration: SetupConfiguration,
    pub choices: Vec<SetupChoice>,
    pub services: Vec<SetupServiceChoice>,
    pub connection_url: Option<String>,
    pub external_bot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_status: Option<String>,
    pub admission: Option<crate::bots::BotAdmission>,
    pub confirmation_digest: Option<String>,
    pub result: Option<SetupResult>,
    pub notice: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupChoice {
    pub id: String,
    pub label: String,
    pub selected: bool,
}
impl SetupChoice {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            selected: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SetupIdentity {
    pub grant_id: Option<String>,
    pub subject: Option<String>,
    pub display_name: Option<String>,
    pub login_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupOwnerChoice {
    pub owner: ResourceOwner,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupServiceChoice {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub connected: bool,
    pub selected: bool,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupExternalBot {
    pub id: String,
    pub name: String,
    pub username: String,
    pub connection_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupResult {
    pub bot_id: String,
    pub state: String,
    pub operation: Option<String>,
    pub failure: Option<String>,
    pub sandbox_id: Option<String>,
    pub workspace_id: Option<String>,
    pub agent_id: Option<String>,
    pub bot_url: Option<String>,
}

/// Durable creation identity; no platform options or credentials belong in this record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupConnectionRequest {
    pub record_id: String,
    pub source_event_id: String,
    pub source_event_ref: String,
    pub authority_stamp: String,
    pub plan_digest: String,
    pub connection_ref: Option<String>,
    pub status: String,
    pub setup_url: Option<String>,
}
#[derive(Debug, Clone)]
pub struct SetupConnectionStatus {
    pub connection_ref: String,
    pub status: String,
    pub setup_url: Option<String>,
}
impl SetupConnectionRequest {
    pub(super) fn terminal(&self) -> bool {
        matches!(self.status.as_str(), "failed" | "cancelled" | "expired")
    }
}
