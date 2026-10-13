//! Absolute settings for an idle Agent and durable runtime confirmation.
use super::Agents;
use crate::{
    Error, IdempotencyKey, Result,
    profiles::{PermissionLevel, ThinkingEffort},
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// All settings must be supplied; the immutable creation profile is preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAgentSettingsRequest {
    /// Exact model ID, at most 256 bytes and without control characters.
    pub model: String,
    /// Desired reasoning effort.
    pub reasoning_effort: ThinkingEffort,
    /// Desired runtime permission mode.
    pub permission_mode: PermissionLevel,
}
/// Durable desired settings and their runtime confirmation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSettings {
    /// Exact model ID; empty for historical profiles that defer model selection.
    pub model: String,
    /// Desired reasoning effort.
    pub reasoning_effort: ThinkingEffort,
    /// Desired runtime permission mode.
    pub permission_mode: PermissionLevel,
    /// True while the runtime acknowledgement is still outstanding.
    pub pending: bool,
}
impl Agents<'_> {
    /// Read the Agent owner's settings and whether recovery is still pending.
    pub async fn settings(&self, id: &str) -> Result<AgentSettings> {
        self.client.get(&["agents", id, "settings"], &[]).await
    }
    /// Update an idle Agent. Retry uncertain replies with this exact body and key.
    pub async fn update_settings(
        &self,
        id: &str,
        body: &UpdateAgentSettingsRequest,
        key: &IdempotencyKey,
    ) -> Result<AgentSettings> {
        if body.model.is_empty()
            || body.model.len() > 256
            || body.model.trim() != body.model
            || body.model.chars().any(char::is_control)
        {
            return Err(Error::validation("invalid Agent model"));
        }
        self.client
            .command(
                Method::PATCH,
                &["agents", id, "settings"],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await
    }
}
