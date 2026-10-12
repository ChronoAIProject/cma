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
    pub contexts: &'a dyn super::authority::ContextStore,
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
            || context.source["platform"]
                .as_str()
                .is_none_or(|platform| platform.is_empty() || platform.len() > 32)
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
        if activity["actor"]["kind"].as_str() != Some("human")
            && !is_connection_notification(activity)
        {
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
                platform: context.source["platform"]
                    .as_str()
                    .ok_or(BotError::Forbidden)?
                    .into(),
            },
            event_id: context.event_id,
            reference: reference.into(),
            occurred_at: activity["occurred_at"]
                .as_str()
                .ok_or(BotError::Forbidden)?
                .into(),
            input,
        })
    }
}
pub(super) fn is_connection_notification(activity: &serde_json::Value) -> bool {
    activity["actor"]["kind"].as_str() == Some("system")
        && activity["kind"]["type"].as_str() == Some("event")
        && activity["kind"]["ce"]["type"].as_str() == Some("gateway.connection.ready")
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
        Some("event") => Ok(SetupInput::Notification),
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

impl SetupAuthority {
    pub(super) async fn verified_event(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
        activity: Option<&serde_json::Value>,
    ) -> Result<SetupEvent, BotError> {
        if reference.starts_with("cma1.cont.") {
            return self
                .continuation_event(binding, credential, reference)
                .await;
        }
        self.require_binding(binding, credential).await.map_err(|error| {
            tracing::warn!(stage = "binding_before_gateway", error = ?error, "Bot Father binding authorization failed");
            error
        })?;
        let event = EventVerifier {
            http: &self.http,
            gateway: &self.gateway,
            binding: &self.binding,
            contexts: self.contexts.as_ref(),
        }
        .verify(binding, credential, reference, activity)
        .await?;
        self.require_binding(binding, credential).await.map_err(|error| {
            tracing::warn!(stage = "binding_after_gateway", error = ?error, "Bot Father binding authorization failed after gateway verification");
            error
        })?;
        let origin = self.origin_for_event(&event, reference, credential).await?;
        self.auth
            .ensure_actor_link_with_origin(&super::authorization::actor(&event.actor), origin)
            .await?;
        Ok(event)
    }

    async fn continuation_event(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
    ) -> Result<SetupEvent, BotError> {
        self.require_binding(binding, credential).await?;
        let (linked_actor, origin) = self.auth.authorized_actor_continuation(reference).await?;
        if linked_actor.binding_id != binding || binding != self.binding {
            return Err(BotError::Forbidden);
        }
        let actor = ExternalActor {
            binding_id: linked_actor.binding_id,
            channel_id: linked_actor.channel_id,
            conversation_id: linked_actor.conversation_id,
            actor_id: linked_actor.actor_id,
            platform: linked_actor.platform,
        };
        let (current, conversation, source) = self
            .admitted_event(
                &actor,
                &origin.source_event.event_id,
                &origin.source_event.reference,
                credential,
            )
            .await?;
        if current.generation != origin.binding_generation
            || conversation.agent_id != origin.agent_id
            || conversation.worktree_id != origin.worktree_id
            || conversation.conversation_id != origin.provider_conversation_id
            || source != origin.source_event
        {
            return Err(BotError::Forbidden);
        }
        // Preserve the original signed input and timestamp. Login completion
        // must never be mistaken for fresh human creation consent.
        let activity = &source.payload["activity"];
        Ok(SetupEvent {
            actor,
            event_id: source.event_id.clone(),
            reference: source.reference.clone(),
            occurred_at: activity["occurred_at"]
                .as_str()
                .ok_or(BotError::Forbidden)?
                .into(),
            input: setup_input(&activity["kind"])?,
        })
    }

    pub(super) async fn source_reference(&self, reference: &str) -> Result<String, BotError> {
        if reference.starts_with("cma1.cont.") {
            Ok(self
                .auth
                .authorized_actor_continuation(reference)
                .await?
                .1
                .source_event
                .reference)
        } else {
            Ok(reference.into())
        }
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
        let context = self.contexts.get(binding, conversation, event).await?;
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
