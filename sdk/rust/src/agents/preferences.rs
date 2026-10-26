//! Mutable owner preferences with revision and runtime capability checks.
use super::Agents;
use crate::{Error, IdempotencyKey, Result};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// Owner-visible settings; managed deployment instructions remain server-owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPreferences {
    /// Explicit owner title or the existing provider-generated title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Exact editable owner instructions.
    pub system_prompt: String,
    /// True until the original native conversation acknowledges the update.
    pub prompt_pending: bool,
    /// Whether the current runtime supports an effective instruction update.
    pub prompt_supported: bool,
    /// Revision for a subsequent compare-and-swap update.
    pub revision: u64,
}

/// Change exactly one preference; replay uncertain results with the same key/body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAgentPreferencesRequest {
    /// Last observed preference revision.
    pub expected_revision: u64,
    /// New title, at most 64 Unicode characters after trimming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Replacement owner instructions, at most 32 KiB UTF-8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}
impl Agents<'_> {
    /// Read current owner preferences and native update support.
    pub async fn preferences(&self, id: &str) -> Result<AgentPreferences> {
        self.client.get(&["agents", id, "preferences"], &[]).await
    }
    /// Rename or change instructions without changing the immutable creation profile.
    pub async fn update_preferences(
        &self,
        id: &str,
        body: &UpdateAgentPreferencesRequest,
        key: &IdempotencyKey,
    ) -> Result<AgentPreferences> {
        let valid = match (&body.title, &body.system_prompt) {
            (Some(title), None) => {
                let title = title.trim();
                !title.is_empty()
                    && title.chars().count() <= 64
                    && !title.chars().any(char::is_control)
            }
            (None, Some(prompt)) => prompt.len() <= 32 * 1024 && !prompt.contains('\0'),
            _ => false,
        };
        if !valid {
            return Err(Error::validation("invalid Agent preferences"));
        }
        self.client
            .command(
                Method::PATCH,
                &["agents", id, "preferences"],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await
    }
}
