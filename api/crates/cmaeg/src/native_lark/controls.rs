//! Native interactive transport: own-app sends and exact-message PATCH only.
use super::{Client, Error};
use cmaeg_core::channel::{NativeLarkReply, Secret};
use reqwest::Method;
use serde_json::{json, Value};
fn content(card: &Value) -> Result<String, Error> {
    let text = serde_json::to_string(card).map_err(|_| Error::Invalid)?;
    if card["schema"] != "2.0" || card["config"]["update_multi"] != true || text.len() > 28 * 1024 {
        return Err(Error::Invalid);
    }
    Ok(text)
}
fn bounded(body: &Value) -> Result<(), Error> {
    if serde_json::to_vec(body).map_err(|_| Error::Invalid)?.len() > 30 * 1024 {
        return Err(Error::Invalid);
    }
    Ok(())
}
impl Client {
    pub(crate) async fn send_card(
        &self,
        token: &Secret,
        reply: &NativeLarkReply,
        card: &Value,
    ) -> Result<String, Error> {
        let content = content(card)?;
        let body = if reply.thread_id.is_some() {
            json!({"msg_type":"interactive","content":content,"reply_in_thread":true})
        } else {
            json!({"receive_id":reply.chat_id,"msg_type":"interactive","content":content})
        };
        bounded(&body)?;
        self.send_content(token, reply, "interactive", card.clone())
            .await
    }
    pub(crate) async fn card_message(
        &self,
        token: &Secret,
        reply: &NativeLarkReply,
    ) -> Result<(), Error> {
        reply.validate().map_err(|_| Error::Invalid)?;
        let value = self
            .call(
                &reply.identity,
                Method::GET,
                &format!(
                    "/open-apis/im/v1/messages/{}?user_id_type=open_id",
                    reply.message_id
                ),
                Some(token),
                None,
            )
            .await?;
        let rows = value["data"]["items"]
            .as_array()
            .filter(|r| r.len() == 1)
            .ok_or(Error::Rejected)?;
        let message = &rows[0];
        let thread = match message.get("thread_id") {
            None => None,
            Some(Value::String(id)) => (!id.is_empty()).then_some(id.as_str()),
            _ => return Err(Error::Rejected),
        };
        if message["message_id"].as_str() != Some(&reply.message_id)
            || message["chat_id"].as_str() != Some(&reply.chat_id)
            || message["msg_type"] != "interactive"
            || message["deleted"] != false
            || thread != reply.thread_id.as_deref()
            || message["sender"]["sender_type"] != "app"
            || message["sender"]["id_type"] != "app_id"
            || message["sender"]["id"].as_str() != Some(&reply.identity.app_id)
            || message["sender"]["tenant_key"].as_str() != Some(&reply.identity.tenant_key)
        {
            return Err(Error::Rejected);
        }
        Ok(())
    }
    pub(crate) async fn patch_card(
        &self,
        token: &Secret,
        reply: &NativeLarkReply,
        card: &Value,
    ) -> Result<(), Error> {
        reply.validate().map_err(|_| Error::Invalid)?;
        let body = json!({"content":content(card)?});
        bounded(&body)?;
        let value = self
            .call(
                &reply.identity,
                Method::PATCH,
                &format!("/open-apis/im/v1/messages/{}", reply.message_id),
                Some(token),
                Some(body),
            )
            .await?;
        if value["data"].as_object().is_none_or(|v| !v.is_empty()) {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
}
