//! Canonical signed-envelope facts, stripped of the platform Flow token.
use super::values::{self, Submitted};
use crate::whatsapp::webhook::{Content, Error, Message};
use cmaeg_core::{activity::SourceKind, channel::Source};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Callback {
    pub kind: String,
    pub card: String,
    pub revision: u64,
    pub source: SourceKind,
    pub id: String,
    pub sender: String,
    pub context: String,
    pub timestamp: i64,
    pub fields: Submitted,
}
pub(crate) fn extract(raw: &Value) -> Result<Content, Error> {
    let reply = &raw["interactive"]["nfm_reply"];
    if raw["interactive"]["type"] != "nfm_reply"
        || reply["name"] != "flow"
        || raw["interactive"].get("button_reply").is_some()
        || raw["interactive"].get("list_reply").is_some()
    {
        return Err(Error::Message);
    }
    let encoded = reply["response_json"].as_str().ok_or(Error::Message)?;
    let (token, fields) = values::decode(encoded).map_err(|_| Error::Message)?;
    let (card, revision) =
        crate::interactions::whatsapp_flow::decode(&token).map_err(|_| Error::Message)?;
    let context = raw["context"]["id"]
        .as_str()
        .filter(|id| cmaeg_core::channel::whatsapp::valid_message_id(id))
        .ok_or(Error::Message)?;
    Ok(Content::Flow {
        card,
        revision,
        context: context.into(),
        fields,
    })
}
impl Callback {
    pub(crate) fn from_message(source: &Source, message: &Message) -> Result<Self, Error> {
        let Content::Flow {
            card,
            revision,
            context,
            fields,
        } = &message.content
        else {
            return Err(Error::Message);
        };
        if message.group_id.is_some() {
            return Err(Error::Message);
        }
        Ok(Self {
            kind: "native_whatsapp_flow_completion_v1".into(),
            card: card.clone(),
            revision: *revision,
            source: crate::ingress::whatsapp::source_kind(source).ok_or(Error::Message)?,
            id: message.id.clone(),
            sender: message.sender.clone(),
            context: context.clone(),
            timestamp: message.timestamp,
            fields: fields.clone(),
        })
    }
    pub(crate) fn event_id(&self) -> Result<cmaeg_core::ids::EventId, Error> {
        cmaeg_core::ids::EventId::new(format!(
            "native-whatsapp-flow:{}",
            super::hash(&(&self.source, &self.id))
        ))
        .map_err(|_| Error::Message)
    }
    pub(crate) fn valid(&self) -> bool {
        self.kind == "native_whatsapp_flow_completion_v1"
            && uuid::Uuid::parse_str(&self.card).is_ok()
            && self.revision == 1
            && matches!(self.source, SourceKind::Whatsapp { .. })
            && cmaeg_core::channel::whatsapp::valid_message_id(&self.id)
            && cmaeg_core::channel::whatsapp::valid_message_id(&self.context)
            && cmaeg_core::channel::whatsapp::valid_meta_id(&self.sender)
            && self.timestamp > 0
            && self.fields.len() <= 12
            && serde_json::to_vec(&self.fields).is_ok_and(|v| v.len() <= super::MAX_SUBMITTED_BYTES)
    }
}
