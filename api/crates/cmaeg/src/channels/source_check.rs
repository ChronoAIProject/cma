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
        Source::NativeLark(_) => {
            checks.push(item("credential", "Separate native app credentials", secrets.native_lark.is_some(), "Requires a separately configured single-tenant app, Encrypt Key and Verification Token. Never replace an existing NyxID app webhook. App and bot custody are checked when authored; this diagnostic does not contact Lark/Feishu."));
            checks.push(item("callback", "Native app callback", false, "Subscribe the separate app to receive, recalled and reaction created/deleted events using the native callback URL. Supports text, recall and human reaction deltas for retained messages; incoming edits are unverified."));
            "native_lark".into()
        }
        Source::Whatsapp { group_ids, .. } => {
            checks.push(item("credential", "Native Meta credentials", secrets.whatsapp.is_some(), "Encrypted app, access and verification credentials are required. Meta app and WABA phone custody are checked when these are authored; this diagnostic does not contact Meta."));
            checks.push(item("callback", "Dedicated app callback", false, "Configure the returned native WhatsApp webhook URL on a dedicated Meta app. Existing external callbacks are never changed automatically. Private text and reactions are supported; opted-in existing groups, media and templates require their separate gateway capabilities."));
            if !group_ids.is_empty() {
                checks.push(item("groups", "Existing groups", true, "Explicit group IDs are configured. Phone OBA and group membership are proved at authoring and before group effects; this diagnostic does not contact Meta."));
            }
            "whatsapp".into()
        }
        Source::Discord { .. } => {
            checks.push(item(
                "credential",
                "Discord credential",
                secrets.discord_bot_token.is_some(),
                "Credential presence only; this check does not contact Discord or send messages.",
            ));
            if let Some(activation) = crate::discord::activation::view(state, &loaded.definition) {
                checks.push(item(
                    "native_activation",
                    "Native Discord activation",
                    activation.state == "selected",
                    &activation.detail,
                ));
            }
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
