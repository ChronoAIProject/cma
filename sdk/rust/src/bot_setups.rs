//! Verified Bot Father setup workflow. References and login links are opaque.
use crate::{
    Client, Result,
    bots::BotAdmission,
    profiles::{
        AgentProfileContent, SandboxProfileAvailability, SandboxProfileContent,
        WorkspaceProfileContent,
    },
    resource_access::ResourceOwner,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// Optional values supplied while naming the Bot.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct BotSetupDraft {
    /// Requested Bot label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Requested Agent instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}
/// Complete replacements for the selected customization step.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct BotSetupCustomization {
    /// Agent model, policy and instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentProfileContent>,
    /// Repository and setup commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceProfileContent>,
    /// Sandbox resources and services.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxProfileContent>,
    /// Scheduling policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<SandboxProfileAvailability>,
}
/// Authenticated event with optional setup values. Actor identity is verified by the server.
#[derive(Clone, Serialize, Deserialize)]
pub struct AdvanceBotSetup {
    /// Exact sealed event reference; never decode or log it.
    pub event_ref: String,
    /// Exact original Activity, required before the first context has been retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<serde_json::Value>,
    /// Proposed naming values; not creation authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<BotSetupDraft>,
    /// Proposed profile changes, subject to the current step and validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customization: Option<BotSetupCustomization>,
}
/// Full effective configuration shown before creation approval.
#[derive(Clone, Serialize, Deserialize)]
pub struct BotSetupConfiguration {
    /// Agent content.
    pub agent: AgentProfileContent,
    /// Workspace content.
    pub workspace: WorkspaceProfileContent,
    /// Sandbox content.
    pub sandbox: SandboxProfileContent,
    /// Scheduling policy.
    pub availability: SandboxProfileAvailability,
}
/// Linked identity status; login URLs and grant IDs should not be logged.
#[derive(Clone, Serialize, Deserialize)]
pub struct BotSetupIdentity {
    /// Confirmed credential grant.
    pub grant_id: Option<String>,
    /// Linked NyxID subject.
    pub subject: Option<String>,
    /// User-visible name.
    pub display_name: Option<String>,
    /// Browser login link, when confirmation is required.
    pub login_url: Option<String>,
}
/// A current step's allowed choice.
#[derive(Clone, Serialize, Deserialize)]
pub struct BotSetupChoice {
    /// Opaque action identifier.
    pub id: String,
    /// User-visible label.
    pub label: String,
    /// Current selection.
    pub selected: bool,
}
/// Service scoped to the selected resource owner.
#[derive(Clone, Serialize, Deserialize)]
pub struct BotSetupServiceChoice {
    /// Exact service identity used in the Agent key.
    pub id: String,
    /// Catalogue slug.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Whether the selected owner has connected it.
    pub connected: bool,
    /// Whether included in this plan.
    pub selected: bool,
    /// Whether required by the integration.
    pub required: bool,
}
/// Provisioning result, observed again through an authenticated event.
#[derive(Clone, Serialize, Deserialize)]
pub struct BotSetupResult {
    /// Durable Bot ID.
    pub bot_id: String,
    /// Current provisioning state.
    pub state: String,
    /// Current operation, when pending.
    pub operation: Option<String>,
    /// User-visible failure, if any.
    pub failure: Option<String>,
    /// Created Sandbox ID.
    pub sandbox_id: Option<String>,
    /// Created Workspace ID.
    pub workspace_id: Option<String>,
    /// Created Agent ID.
    pub agent_id: Option<String>,
    /// Link to the resulting Bot.
    pub bot_url: Option<String>,
}
/// Current setup plan and choices; receiving this view does not imply creation approval.
#[derive(Clone, Serialize, Deserialize)]
pub struct BotSetupView {
    /// Stable setup identity.
    pub setup_id: String,
    /// Current optimistic concurrency revision.
    pub revision: u64,
    /// Current step; unknown future values must not imply completion.
    pub step: String,
    /// Current revision-bound interaction identity.
    pub interaction_id: String,
    /// Linked human identity.
    pub identity: BotSetupIdentity,
    /// Selected personal or organization owner.
    pub owner: Option<ResourceOwner>,
    /// Proposed Bot name.
    pub name: Option<String>,
    /// Full effective configuration.
    pub configuration: BotSetupConfiguration,
    /// Allowed current actions.
    pub choices: Vec<BotSetupChoice>,
    /// Current owner-scoped service choices.
    pub services: Vec<BotSetupServiceChoice>,
    /// Required browser login or service connection link.
    pub connection_url: Option<String>,
    /// Selected NyxID Telegram bot.
    pub external_bot_id: Option<String>,
    /// Selected message admission policy.
    pub admission: Option<BotAdmission>,
    /// Result after confirmed creation.
    pub result: Option<BotSetupResult>,
    /// Current user-visible notice.
    pub notice: Option<String>,
}
/// Provider workflow requiring the configured Father binding's Agent key.
pub struct BotSetups<'a>(&'a Client);
impl Client {
    /// Advance a verified Bot Father setup flow.
    pub fn bot_setups(&self) -> BotSetups<'_> {
        BotSetups(self)
    }
}
impl BotSetups<'_> {
    /// Advance with the exact sealed event and original Activity.
    pub async fn advance(&self, body: &AdvanceBotSetup) -> Result<BotSetupView> {
        self.0
            .operation(
                Method::POST,
                &["bot-setups", "advance"],
                Some(body),
                StatusCode::OK,
            )
            .await
    }
}
