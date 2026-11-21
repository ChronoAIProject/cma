//! Native controls use only the stored-card renderer and authenticated callback locators.
use super::{webhook, Client, Error};
use cmaeg_core::channel::{
    whatsapp::{valid_message_id, valid_meta_id},
    WhatsappCredentials,
};
use serde_json::{json, Value};

impl Client {
    pub(crate) async fn send_interactive(
        &self,
        credentials: &WhatsappCredentials,
        phone: &str,
        recipient: &str,
        parent: &str,
        prepared: &crate::interactions::PreparedCard,
        uploaded: Option<super::outbound::UploadedMedia>,
    ) -> Result<String, Error> {
        if !valid_meta_id(phone) || !valid_meta_id(recipient) || !valid_message_id(parent) {
            return Err(Error::Invalid);
        }
        let (mut interactive, _) = crate::interactions::whatsapp::render(
            &prepared.card.message,
            &prepared.card.id,
            prepared.card.revision,
        )
        .map_err(|_| Error::Invalid)?;
        if let Some(uploaded) = uploaded {
            if interactive["type"] != "button" || prepared.card.native_whatsapp.is_none() {
                return Err(Error::Invalid);
            }
            interactive["header"] = uploaded.into_interactive_header(phone)?;
        }
        let result = self
            .call(
                reqwest::Method::POST,
                Self::endpoint(&format!("{phone}/messages"))?,
                credentials.access_token.as_str(),
                Some(json!({
                    "messaging_product":"whatsapp", "recipient_type":"individual", "to":recipient,
                    "context":{"message_id":parent}, "type":"interactive", "interactive":interactive
                })),
            )
            .await?;
        Self::message_identity(&result, recipient)
    }
}

pub(super) fn extract(raw: &Value) -> Result<webhook::Content, webhook::Error> {
    if raw["interactive"]["type"] == "nfm_reply" {
        return super::forms::callback::extract(raw);
    }
    let interactive = &raw["interactive"];
    let kind = interactive["type"]
        .as_str()
        .ok_or(webhook::Error::Message)?;
    let prefix = match kind {
        "button_reply" if interactive.get("list_reply").is_none() => "cw1b:",
        "list_reply" if interactive.get("button_reply").is_none() => "cw1l:",
        _ => return Err(webhook::Error::Message),
    };
    let locator = interactive[kind]["id"]
        .as_str()
        .filter(|id| id.starts_with(prefix) && id.len() <= 64 && !id.chars().any(char::is_control))
        .ok_or(webhook::Error::Message)?;
    let context = raw["context"]["id"]
        .as_str()
        .filter(|id| valid_message_id(id))
        .ok_or(webhook::Error::Message)?;
    // Foreign controls are explicitly refused. Titles never become action values;
    // context.from is a phone address, not the metadata phone-number resource ID.
    Ok(webhook::Content::Control {
        locator: locator.into(),
        context: context.into(),
    })
}
