//! Static contact/location replies retain the original private event destination.
use super::{Client, Error};
use crate::{channels::resolver::ResolvedChannel, records::RecordClient};
use cmaeg_core::{
    activity::{ConversationKind, SourceKind},
    channel::{
        whatsapp::{valid_message_id, valid_meta_id},
        WhatsappCredentials,
    },
    message::StructuredContent,
    seal::{EventRefPayload, MasterKeyRing, ReplyAddress},
};
use reqwest::Method;
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) async fn authorized(
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
) -> bool {
    let ReplyAddress::Whatsapp {
        app_id,
        waba_id,
        phone_number_id,
        recipient_id,
        ..
    } = &event.reply
    else {
        return false;
    };
    if super::credentials(&channel.definition, &event.reply)
        .is_none_or(|credentials| credentials.validate().is_err())
    {
        return false;
    }
    let Ok(plan) =
        crate::turns::routed::retained_received_source(records, keys, channel, event).await
    else {
        return false;
    };
    matches!(&plan.activity.source, SourceKind::Whatsapp { app_id: app, waba_id: account, phone_number_id: phone }
        if app == app_id && account == waba_id && phone == phone_number_id)
        && plan.activity.conversation.kind == ConversationKind::Private
        && plan.activity.conversation.id == *recipient_id
        && plan.activity.conversation.thread_id.is_none()
        && plan.activity.actor.id == *recipient_id
}

/// Prepared before the durable send gate; no arbitrary JSON or recipient override.
pub(crate) struct PreparedStructured {
    phone: String,
    recipient: String,
    body: Value,
}

impl PreparedStructured {
    pub(crate) fn new(
        phone: &str,
        recipient: &str,
        parent: &str,
        content: &StructuredContent,
    ) -> Result<Self, Error> {
        if !valid_meta_id(phone) || !valid_meta_id(recipient) || !valid_message_id(parent) {
            return Err(Error::Invalid);
        }
        content.validate().map_err(|_| Error::Invalid)?;
        let (kind, value) = match content {
            StructuredContent::Contact {
                phone_number,
                first_name,
                last_name,
                vcard: None,
            } => {
                let formatted = last_name
                    .as_ref()
                    .map_or_else(|| first_name.clone(), |last| format!("{first_name} {last}"));
                let mut name = json!({"formatted_name": formatted, "first_name": first_name});
                if let Some(last) = last_name {
                    name["last_name"] = last.clone().into();
                }
                (
                    "contacts",
                    json!([{"name": name, "phones": [{"phone": phone_number}]}]),
                )
            }
            StructuredContent::Location {
                latitude,
                longitude,
                horizontal_accuracy: None,
            } => (
                "location",
                json!({"latitude":latitude.to_string(), "longitude":longitude.to_string()}),
            ),
            StructuredContent::Venue {
                latitude,
                longitude,
                title,
                address,
            } => (
                "location",
                json!({"latitude":latitude.to_string(), "longitude":longitude.to_string(), "name":title, "address":address}),
            ),
            // Meta has no vCard or accuracy member in these message payloads.
            // Explicit unsupported data must never be silently discarded.
            _ => return Err(Error::Invalid),
        };
        let mut body = json!({"messaging_product":"whatsapp", "recipient_type":"individual", "to":recipient,
            "context":{"message_id":parent}, "type":kind});
        body[kind] = value;
        if serde_json::to_vec(&body).map_err(|_| Error::Invalid)?.len() > 32 * 1024 {
            return Err(Error::Invalid);
        }
        Ok(Self {
            phone: phone.into(),
            recipient: recipient.into(),
            body,
        })
    }
}

impl Client {
    /// One Graph attempt. Original receipt tracking and replay belong to delivery.
    pub(crate) async fn send_structured(
        &self,
        credentials: &WhatsappCredentials,
        prepared: PreparedStructured,
    ) -> Result<String, Error> {
        credentials.validate().map_err(|_| Error::Invalid)?;
        let result = self
            .call(
                Method::POST,
                Self::endpoint(&format!("{}/messages", prepared.phone))?,
                credentials.access_token.as_str(),
                Some(prepared.body),
            )
            .await?;
        Self::message_identity(&result, &prepared.recipient)
    }
}
