use super::{
    history_authority::Authority,
    history_projection::{check_scope, id, millis},
    Client, Error,
};
use cmaeg_core::{channel::Secret, seal::NativeLarkHistoryCursorPayload};
use reqwest::Method;
use serde_json::Value;
impl Client {
    pub(crate) async fn history_anchor(
        &self,
        token: &Secret,
        authority: &Authority,
    ) -> Result<(), Error> {
        let c = &authority.cursor;
        let value = self
            .call(
                &c.origin.identity,
                Method::GET,
                &format!(
                    "/open-apis/im/v1/messages/{}?user_id_type=open_id",
                    c.origin.message_id
                ),
                Some(token),
                None,
            )
            .await?;
        let items = value["data"]["items"]
            .as_array()
            .filter(|v| v.len() == 1)
            .ok_or(Error::Rejected)?;
        let row = &items[0];
        check_scope(row, c)?;
        let thread = row
            .get("thread_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        if id(&row["message_id"], "om_")? != c.origin.message_id
            || row["deleted"] != false
            || thread != c.origin.thread_id.as_deref()
            || millis(&row["create_time"])? != c.anchor_created_ms
            || row["sender"]["sender_type"] != "user"
            || row["sender"]["id_type"] != "open_id"
            || row["sender"]["id"].as_str() != Some(authority.activity.actor.id.as_str())
            || row["sender"]["tenant_key"].as_str() != Some(c.origin.identity.tenant_key.as_str())
        {
            return Err(Error::Rejected);
        }
        Ok(())
    }
    pub(crate) async fn history_page(
        &self,
        token: &Secret,
        c: &NativeLarkHistoryCursorPayload,
    ) -> Result<Value, Error> {
        let path = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query
                .append_pair(
                    "container_id_type",
                    if c.origin.thread_id.is_some() {
                        "thread"
                    } else {
                        "chat"
                    },
                )
                .append_pair(
                    "container_id",
                    c.origin.thread_id.as_deref().unwrap_or(&c.origin.chat_id),
                )
                .append_pair("sort_type", "ByCreateTimeDesc")
                .append_pair("page_size", "25");
            if c.origin.thread_id.is_none() {
                query.append_pair("end_time", &(c.anchor_created_ms / 1000 + 1).to_string());
            }
            if !c.page_token.is_empty() {
                query.append_pair("page_token", &c.page_token);
            }
            format!("/open-apis/im/v1/messages?{}", query.finish())
        };
        self.call(&c.origin.identity, Method::GET, &path, Some(token), None)
            .await
    }
}
