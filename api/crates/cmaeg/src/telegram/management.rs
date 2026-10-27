use super::*;

impl TelegramClient {
    pub async fn get_me(&self, token: &Secret) -> Result<BotIdentity, Error> {
        let value = self.call(token, "getMe", &json!({})).await?;
        if value["is_bot"] != true {
            return Err(Error::Unconfirmed);
        }
        let id = positive(&value["id"])?;
        let first_name = value["first_name"]
            .as_str()
            .filter(|name| !name.is_empty() && name.len() <= 256)
            .ok_or(Error::Unconfirmed)?
            .to_owned();
        let username = value
            .get("username")
            .map(|name| {
                name.as_str()
                    .filter(|name| {
                        (5..=32).contains(&name.len())
                            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    })
                    .map(str::to_owned)
                    .ok_or(Error::Unconfirmed)
            })
            .transpose()?;
        Ok(BotIdentity {
            id,
            username,
            first_name,
            can_manage_bots: value["can_manage_bots"] == true,
        })
    }
    pub async fn managed_bot_token(
        &self,
        manager: &Secret,
        child_bot_id: &str,
    ) -> Result<Secret, Error> {
        let id = child_bot_id
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or(Error::Rejected)?;
        let value = self
            .call(manager, "getManagedBotToken", &json!({"user_id":id}))
            .await?;
        let token = value
            .as_str()
            .filter(|token| {
                token.split_once(':').is_some_and(|(prefix, key)| {
                    prefix == child_bot_id
                        && !key.is_empty()
                        && key.len() <= 256
                        && key
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                })
            })
            .ok_or(Error::Unconfirmed)?;
        Ok(Secret::new(token))
    }
    pub async fn send_managed_keyboard(
        &self,
        token: &Secret,
        chat_id: &str,
        request_id: i32,
        suggested_name: Option<&str>,
        suggested_username: Option<&str>,
    ) -> Result<String, Error> {
        if !chat_id.parse::<i64>().is_ok_and(|id| id > 0) {
            return Err(Error::Rejected);
        }
        let mut request = json!({"request_id":request_id});
        if let Some(name) = suggested_name {
            request["suggested_name"] = name.into();
        }
        if let Some(username) = suggested_username {
            request["suggested_username"] = username.into();
        }
        let markup = json!({"keyboard":[[{"text":"Create a bot","request_managed_bot":request}]],"resize_keyboard":true,"one_time_keyboard":true,"selective":true});
        self.send_message(
            token,
            chat_id,
            None,
            "Use the button to create a Telegram bot.",
            Some(&markup),
        )
        .await
    }
    pub async fn delete_webhook(&self, token: &Secret) -> Result<(), Error> {
        if self
            .call(
                token,
                "deleteWebhook",
                &json!({"drop_pending_updates":false}),
            )
            .await?
            != true
        {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
    pub async fn get_webhook_url(&self, token: &Secret) -> Result<String, Error> {
        let value = self.call(token, "getWebhookInfo", &json!({})).await?;
        value["url"]
            .as_str()
            .filter(|url| url.len() <= 8192)
            .map(str::to_owned)
            .ok_or(Error::Unconfirmed)
    }
    pub async fn set_webhook(
        &self,
        token: &Secret,
        url: &str,
        secret: &Secret,
    ) -> Result<(), Error> {
        let parsed = Url::parse(url).map_err(|_| Error::Rejected)?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err(Error::Rejected);
        }
        if secret.as_str().is_empty()
            || secret.as_str().len() > 256
            || !secret
                .as_str()
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(Error::Rejected);
        }
        let value = self.call(token, "setWebhook", &json!({"url":url,"secret_token":secret.as_str(),"allowed_updates":["message","edited_message","channel_post","edited_channel_post","callback_query","message_reaction","message_reaction_count"],"drop_pending_updates":false})).await?;
        if value != true {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
}
