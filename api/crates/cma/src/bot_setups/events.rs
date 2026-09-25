use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct Context {
    channel_id: String,
    provider_binding_id: String,
    conversation_id: String,
    event_id: String,
    source: serde_json::Value,
    activity_verified: bool,
    activity: serde_json::Value,
}
impl SetupDependencies {
    pub(super) async fn verified_event(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
    ) -> Result<SetupEvent, BotError> {
        if binding != self.binding
            || reference.len() > 4096
            || !reference.starts_with("cmaeg1.ev.")
            || !reference
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            return Err(BotError::Forbidden);
        }
        let context: Context = serde_json::from_value(
            self.json(
                self.http
                    .get(format!("{}/v1/events/{reference}/context", self.gateway))
                    .bearer_auth(credential),
            )
            .await?,
        )
        .map_err(|_| BotError::Unavailable)?;
        if context.provider_binding_id != binding
            || !context.activity_verified
            || (context.source["type"].as_str() != Some("nyxid_relay")
                || context.source["platform"].as_str() != Some("telegram"))
        {
            return Err(BotError::Forbidden);
        }
        let activity = &context.activity;
        if activity["actor"]["kind"].as_str() != Some("human") {
            return Err(BotError::Forbidden);
        }
        let actor_id = activity["actor"]["id"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or(BotError::Forbidden)?
            .to_owned();
        // Linking and configuration use private conversations. Group callbacks
        // do not expose login links or another member's private configuration.
        if activity["conversation"]["kind"].as_str() != Some("private") {
            return Err(BotError::Invalid(
                "Open a private conversation with Bot Father to create a Bot.".into(),
            ));
        }
        let kind = &activity["kind"];
        let input = match kind["type"].as_str() {
            Some("message") => SetupInput::Text(kind["text"].as_str().unwrap_or_default().into()),
            Some("command") if kind["name"].as_str() == Some("start") => SetupInput::Text(format!(
                "/start {}",
                kind["args"].as_str().unwrap_or_default()
            )),
            Some("interaction") => SetupInput::Interaction {
                block_id: kind["block_id"].as_str().ok_or(BotError::Forbidden)?.into(),
                action_id: kind["action_id"]
                    .as_str()
                    .ok_or(BotError::Forbidden)?
                    .into(),
            },
            _ => {
                return Err(BotError::Invalid(
                    "This event cannot configure a Bot.".into(),
                ))
            }
        };
        Ok(SetupEvent {
            actor: ExternalActor {
                binding_id: binding.into(),
                channel_id: context.channel_id,
                conversation_id: context.conversation_id,
                actor_id,
                platform: "telegram".into(),
            },
            event_id: context.event_id,
            input,
        })
    }
}
