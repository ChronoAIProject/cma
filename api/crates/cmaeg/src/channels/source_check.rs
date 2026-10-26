//! Read-only diagnostics for one saved producer; never an external delivery probe.
use super::management::{load, ManagementError, ManagementState};
use crate::auth::Creator;
use cmaeg_core::{
    channel::{CustomAuth, Source},
    ids::ChannelId,
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct SourceCheckResponse {
    pub source_index: usize,
    pub platform: String,
    pub scope: String,
    pub external_delivery_tested: bool,
    pub checks: Vec<SourceCheck>,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct SourceCheck {
    pub id: String,
    pub label: String,
    pub status: SourceCheckStatus,
    pub detail: String,
}
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceCheckStatus {
    Passed,
    Unavailable,
}

pub async fn check(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
    index: usize,
) -> Result<SourceCheckResponse, ManagementError> {
    let loaded = load(state, creator, channel_id).await?;
    let source = loaded
        .definition
        .sources
        .get(index)
        .ok_or(ManagementError::NotFound)?;
    let secrets = &loaded.definition.secrets;
    let mut checks = vec![item(
        "configuration",
        "Saved configuration",
        true,
        "The saved channel definition passes schema and policy validation.",
    )];
    let platform = match source {
        Source::NyxidRelay {
            platform,
            route_ids,
            ..
        } => {
            checks.push(item("routes", "Attached routes", !route_ids.is_empty(),
                if route_ids.is_empty() { "No relay route is attached." } else { "Relay route identifiers are configured; upstream ownership and availability were not tested." }));
            checks.push(item(
                "credential",
                "Relay credential",
                secrets.channel_key.is_some(),
                if secrets.channel_key.is_some() {
                    "A relay credential is retained securely; its upstream validity was not tested."
                } else {
                    "A relay credential is not configured."
                },
            ));
            platform.as_str().to_owned()
        }
        Source::Discord { .. } => {
            checks.push(item(
                "credential",
                "Discord credential",
                secrets.discord_bot_token.is_some(),
                "Credential presence only; live Gateway intake and replies are not yet activated.",
            ));
            "discord".into()
        }
        Source::Telegram { .. } => {
            checks.push(item(
                "credential",
                "Telegram credential",
                secrets.telegram_bot_token.is_some(),
                if secrets.telegram_bot_token.is_some() {
                    "A bot credential is retained securely; Telegram authentication was not tested."
                } else {
                    "A Telegram bot credential is not configured."
                },
            ));
            checks.push(item("webhook", "Webhook verification", secrets.telegram_webhook_secret.is_some(),
                if secrets.telegram_webhook_secret.is_some() { "A webhook verification secret is configured; webhook registration was not tested." } else { "A webhook verification secret is not configured." }));
            "telegram".into()
        }
        Source::Custom { auth, .. } => {
            let present = match auth {
                CustomAuth::Hmac { key_id } => {
                    secrets.hmac_secrets.iter().any(|(id, _)| id == key_id)
                }
                CustomAuth::NyxidKey { key_hashes } => !key_hashes.is_empty(),
            };
            checks.push(item("credential", "Authentication configuration", present,
                if present { "The configured authentication references are present; caller credentials were not tested." } else { "The configured authentication reference is unavailable." }));
            "custom".into()
        }
    };
    checks.push(item("external_delivery", "External delivery", false,
        "Send a message from the configured platform to verify incoming delivery. This check sends no external messages."));
    Ok(SourceCheckResponse {
        source_index: index,
        platform,
        scope: "configuration_only".into(),
        external_delivery_tested: false,
        checks,
    })
}
fn item(id: &str, label: &str, passed: bool, detail: &str) -> SourceCheck {
    SourceCheck {
        id: id.into(),
        label: label.into(),
        status: if passed {
            SourceCheckStatus::Passed
        } else {
            SourceCheckStatus::Unavailable
        },
        detail: detail.into(),
    }
}
