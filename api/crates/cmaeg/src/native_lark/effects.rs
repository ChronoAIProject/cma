//! Official regional message endpoints, with exact sender/destination receipts.
use super::{Client, Error};
use crate::tools::message_effects::lark_ledger::Reaction;
use cmaeg_core::{
    channel::{lark_resources::safe_id, Secret},
    seal::native_lark_effect::{NativeLarkEffectAuthority, NativeLarkEffectTarget},
};
use reqwest::Method;
use serde_json::{json, Value};

impl Client {
    pub(crate) async fn effect_message(
        &self,
        token: &Secret,
        authority: &NativeLarkEffectAuthority,
        editing: bool,
    ) -> Result<(), Error> {
        let value = self
            .call(
                &authority.reply.identity,
                Method::GET,
                &format!("{}?user_id_type=open_id", path(authority)),
                Some(token),
                None,
            )
            .await?;
        let rows = value
            .pointer("/data/items")
            .and_then(Value::as_array)
            .filter(|rows| rows.len() == 1)
            .ok_or(Error::Rejected)?;
        if !exact_message(&rows[0], authority, editing) {
            return Err(Error::Rejected);
        }
        Ok(())
    }
    pub(crate) async fn effect_delete(
        &self,
        token: &Secret,
        authority: &NativeLarkEffectAuthority,
    ) -> Result<(), Error> {
        if !authority.confirmed() {
            return Err(Error::Invalid);
        }
        self.call(
            &authority.reply.identity,
            Method::DELETE,
            &path(authority),
            Some(token),
            None,
        )
        .await?;
        Ok(())
    }
    pub(crate) async fn effect_edit(
        &self,
        token: &Secret,
        authority: &NativeLarkEffectAuthority,
        text: &str,
    ) -> Result<(), Error> {
        if !authority.editable() || text.is_empty() || text.len() > 28 * 1024 {
            return Err(Error::Invalid);
        }
        let value = self
            .call(
                &authority.reply.identity,
                Method::PUT,
                &path(authority),
                Some(token),
                Some(json!({"msg_type":"text","content":json!({"text":text}).to_string()})),
            )
            .await?;
        if !exact_message(&value["data"], authority, true)
            || value
                .pointer("/data/body/content")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .is_none_or(|body| body["text"].as_str() != Some(text))
        {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
    pub(crate) async fn effect_react(
        &self,
        token: &Secret,
        authority: &NativeLarkEffectAuthority,
        emoji: &str,
    ) -> Result<Reaction, Error> {
        let value = self
            .call(
                &authority.reply.identity,
                Method::POST,
                &format!("{}/reactions", path(authority)),
                Some(token),
                Some(json!({"reaction_type":{"emoji_type":emoji}})),
            )
            .await?;
        reaction(&value, authority, emoji).ok_or(Error::Unconfirmed)
    }
    pub(crate) async fn effect_clear(
        &self,
        token: &Secret,
        authority: &NativeLarkEffectAuthority,
        previous: &Reaction,
    ) -> Result<(), Error> {
        if !safe_id(&previous.id, 512) {
            return Err(Error::Invalid);
        }
        let value = self
            .call(
                &authority.reply.identity,
                Method::DELETE,
                &format!("{}/reactions/{}", path(authority), previous.id),
                Some(token),
                None,
            )
            .await?;
        if reaction(&value, authority, &previous.emoji).is_none_or(|r| r.id != previous.id) {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
}
fn path(a: &NativeLarkEffectAuthority) -> String {
    format!("/open-apis/im/v1/messages/{}", a.reply.message_id)
}
fn exact_message(row: &Value, a: &NativeLarkEffectAuthority, editing: bool) -> bool {
    let sender = &row["sender"];
    let (kind, id_type, id) = match &a.target {
        NativeLarkEffectTarget::Confirmed { .. } => ("app", "app_id", &a.reply.identity.app_id),
        NativeLarkEffectTarget::Received { sender_id } => ("user", "open_id", sender_id),
    };
    let thread = match row.get("thread_id") {
        None => None,
        Some(Value::String(s)) => (!s.is_empty()).then_some(s.as_str()),
        _ => return false,
    };
    row["message_id"].as_str() == Some(&a.reply.message_id)
        && row["chat_id"].as_str() == Some(&a.reply.chat_id)
        && thread == a.reply.thread_id.as_deref()
        && row["deleted"] == false
        && sender["sender_type"].as_str() == Some(kind)
        && sender["id_type"].as_str() == Some(id_type)
        && sender["id"].as_str() == Some(id)
        && sender["tenant_key"].as_str() == Some(&a.reply.identity.tenant_key)
        && if editing {
            row["msg_type"] == "text"
        } else {
            matches!(
                row["msg_type"].as_str(),
                Some("text" | "post" | "image" | "file" | "audio" | "media")
            )
        }
}
fn reaction(value: &Value, a: &NativeLarkEffectAuthority, emoji: &str) -> Option<Reaction> {
    let data = &value["data"];
    let id = data["reaction_id"].as_str()?;
    (safe_id(id, 512)
        && data.pointer("/operator/operator_type")?.as_str()? == "app"
        && data.pointer("/operator/operator_id")?.as_str()? == a.reply.identity.app_id
        && data.pointer("/reaction_type/emoji_type")?.as_str()? == emoji)
        .then(|| Reaction {
            id: id.into(),
            emoji: emoji.into(),
        })
}
