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
    expires_at: i64,
    activity: serde_json::Value,
}
pub(super) struct EventVerifier<'a> {
    pub http: &'a reqwest::Client,
    pub gateway: &'a str,
    pub binding: &'a str,
    pub contexts: &'a crate::store::bot_setup_contexts::MongoBotSetupContextStore,
}
impl EventVerifier<'_> {
    pub(super) async fn verify(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
        supplied_activity: Option<&serde_json::Value>,
    ) -> Result<SetupEvent, BotError> {
        if binding != self.binding
            || reference.len() > 4096
            || !reference.starts_with("cmaeg1.ev.")
            || !reference
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            tracing::warn!(stage = "reference", configured_binding = %self.binding, request_binding = %binding, "Bot Father event verification rejected the binding or event reference");
            return Err(BotError::Forbidden);
        }
        let url = format!("{}/v1/events/{reference}/context", self.gateway);
        let request = match supplied_activity {
            Some(activity) => self.http.post(url).json(activity),
            None => self.http.get(url),
        };
        let context: Context =
            serde_json::from_value(super::request_json(request.bearer_auth(credential)).await?)
                .map_err(|_| BotError::Unavailable)?;
        if context.provider_binding_id != binding
            || !context.activity_verified
            || (context.source["type"].as_str() != Some("nyxid_relay")
                || context.source["platform"].as_str() != Some("telegram"))
        {
            tracing::warn!(
                stage = "context",
                binding_match = context.provider_binding_id == binding,
                activity_verified = context.activity_verified,
                source_type = ?context.source["type"].as_str(),
                source_platform = ?context.source["platform"].as_str(),
                "Bot Father event context was not admitted"
            );
            return Err(BotError::Forbidden);
        }
        let activity = &context.activity;
        if activity["actor"]["kind"].as_str() != Some("human") {
            tracing::warn!(stage = "actor_kind", "Bot Father event actor was not human");
            return Err(BotError::Forbidden);
        }
        let actor_id = activity["actor"]["id"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                tracing::warn!(
                    stage = "actor_id",
                    "Bot Father event actor had no usable identity"
                );
                BotError::Forbidden
            })?
            .to_owned();
        // Linking and configuration use private conversations. Group callbacks
        // do not expose login links or another member's private configuration.
        if activity["conversation"]["kind"].as_str() != Some("private") {
            tracing::warn!(
                stage = "conversation",
                "Bot Father event was not from a private conversation"
            );
            return Err(BotError::Invalid(
                "Open a private conversation with Bot Father to create a Bot.".into(),
            ));
        }
        // Preserve signed user input. The agent supplies structured workflow
        // actions separately; those actions cannot rewrite consent or identity.
        let input = setup_input(&activity["kind"])?;
        if let Some(activity) = supplied_activity {
            let snapshot = serde_json::json!({
                "schema_version": 1,
                "channel_id": context.channel_id,
                "conversation_id": context.conversation_id,
                "activity": activity,
                "event_ref": reference,
            });
            self.contexts
                .retain(
                    binding,
                    &context.conversation_id,
                    &context.event_id,
                    context.expires_at,
                    &snapshot,
                )
                .await?;
        }
        Ok(SetupEvent {
            actor: ExternalActor {
                binding_id: binding.into(),
                channel_id: context.channel_id,
                conversation_id: context.conversation_id,
                actor_id,
                platform: "telegram".into(),
            },
            event_id: context.event_id,
            occurred_at: activity["occurred_at"]
                .as_str()
                .ok_or(BotError::Forbidden)?
                .into(),
            input,
        })
    }
}
fn setup_input(kind: &serde_json::Value) -> Result<SetupInput, BotError> {
    match kind["type"].as_str() {
        Some("message") => Ok(SetupInput::Text(
            kind["text"].as_str().unwrap_or_default().into(),
        )),
        Some("command") => Ok(SetupInput::Text(
            format!(
                "/{} {}",
                kind["name"].as_str().ok_or(BotError::Forbidden)?,
                kind["args"].as_str().unwrap_or_default()
            )
            .trim_end()
            .into(),
        )),
        Some("interaction") => Ok(SetupInput::Interaction {
            block_id: kind["block_id"].as_str().ok_or(BotError::Forbidden)?.into(),
            action_id: kind["action_id"]
                .as_str()
                .ok_or(BotError::Forbidden)?
                .into(),
        }),
        _ => Err(BotError::Invalid(
            "This event cannot configure a Bot.".into(),
        )),
    }
}

impl SetupDependencies {
    pub(super) async fn verified_event(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
        activity: Option<&serde_json::Value>,
    ) -> Result<SetupEvent, BotError> {
        self.require_binding(binding, credential).await.map_err(|error| {
            tracing::warn!(stage = "binding_before_gateway", error = ?error, "Bot Father binding authorization failed");
            error
        })?;
        let event = EventVerifier {
            http: &self.http,
            gateway: &self.gateway,
            binding: &self.binding,
            contexts: &self.stores.bot_setup_contexts,
        }
        .verify(binding, credential, reference, activity)
        .await?;
        self.require_binding(binding, credential).await.map_err(|error| {
            tracing::warn!(stage = "binding_after_gateway", error = ?error, "Bot Father binding authorization failed after gateway verification");
            error
        })?;
        Ok(event)
    }
    pub(super) async fn retained_context(
        &self,
        binding: &str,
        credential: &str,
        conversation: &str,
        event: &str,
    ) -> Result<Option<serde_json::Value>, BotError> {
        if binding != self.binding {
            return Ok(None);
        }
        // Compatibility contexts predate provider admission. Authenticate the
        // exact current binding before and after reading those retained events.
        self.require_binding(binding, credential).await?;
        let context = self
            .stores
            .bot_setup_contexts
            .get(binding, conversation, event)
            .await?;
        self.require_binding(binding, credential).await?;
        Ok(context)
    }

    async fn require_binding(&self, binding: &str, credential: &str) -> Result<(), BotError> {
        let auth = self
            .auth
            .binding_request(credential.into())
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        if auth
            .binding
            .as_ref()
            .is_some_and(|principal| principal.binding_id() == binding)
        {
            Ok(())
        } else {
            Err(BotError::Forbidden)
        }
    }
}

#[cfg(test)]
#[path = "events/tests.rs"]
mod tests;
