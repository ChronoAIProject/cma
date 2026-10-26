//! Type-3 Gateway component callbacks: exact card authority, durable inbox, prompt ACK.
use super::{
    journal::DurableJournal,
    journal_model::{Authority, Inbox},
    normalize::snowflake,
    GatewayError,
};
use crate::{
    ingress::{
        admission::{admit, AdmissionFacts},
        IngressState,
    },
    interactions::{discord::Identity, discord_consume::DiscordClick, Error},
};
use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, ConversationKind,
        InteractionResult, SourceKind,
    },
    channel::Source,
    ids::EventId,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub(super) struct Runtime {
    pub ingress: IngressState,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub activity: Activity,
    pub identity: Identity,
    pub message_id: String,
}
impl Snapshot {
    pub(super) fn validate(&self, bot: &str, interaction: &str) -> Result<(), GatewayError> {
        if self.identity.bot_id != bot
            || self.activity.event_id.as_str() != format!("discord:{bot}:interaction:{interaction}")
            || self.activity.source != (SourceKind::Discord { bot_id: bot.into() })
            || self.activity.routing.as_ref() != Some(&self.identity.routing)
            || !crate::ingress::native_interaction::shape(&self.activity)
            || self.identity.routing.targets[0].id != self.identity.consumer
            || self.activity.conversation.kind != self.identity.conversation_kind
            || self.activity.conversation.thread_id.is_some()
            || self.activity.thread.is_some()
            || !cmaeg_core::channel::valid_discord_id(&self.message_id)
            || self.activity.validate().is_err()
        {
            return Err(GatewayError::Authority);
        }
        Ok(())
    }
}
impl super::DiscordApi {
    pub(crate) fn with_components(mut self, ingress: IngressState) -> Self {
        self.components = Some(Arc::new(Runtime { ingress }));
        self
    }
    /// No Bot header: the short-lived interaction token is valid only at this
    /// exact official endpoint. Never retry a write whose result is uncertain.
    async fn acknowledge_component(
        &self,
        id: &str,
        token: &str,
        budget: Duration,
    ) -> Result<(), GatewayError> {
        if budget.is_zero()
            || token.is_empty()
            || token.len() > 2048
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            return Err(GatewayError::Protocol);
        }
        let url = self
            .origin
            .join(&format!("interactions/{id}/{token}/callback"))
            .map_err(|_| GatewayError::Protocol)?;
        let response = self
            .client
            .post(url)
            .timeout(budget)
            .json(&json!({"type":6}))
            .send()
            .await
            .map_err(|_| GatewayError::Unavailable)?;
        if response.status().as_u16() == 204 {
            Ok(())
        } else {
            Err(GatewayError::Unavailable)
        }
    }
}
impl DurableJournal {
    pub(super) async fn persist_component(
        &self,
        authority: &Authority,
        event: &super::Dispatch,
    ) -> Result<(), GatewayError> {
        let Some(runtime) = &self.api.components else {
            return Ok(());
        };
        if event.data["type"].as_u64() != Some(3) {
            return Ok(());
        }
        let id = match snowflake(&event.data["id"]) {
            Ok(id) => id,
            Err(_) => return Ok(()),
        };
        let inbox_id = crate::connections::store::index_id(
            "discord-native-message-v1",
            &[&authority.record, &authority.bot_id, id],
        );
        // Replayed durable work retains its original normalized identity and
        // never repeats an initial ACK, even after a lost response or restart.
        if let Some((inbox, 1)) = self.store.load::<Inbox>(&inbox_id, true).await? {
            let bytes = self.verify_inbox(&inbox, authority, id).await?;
            return self.observe_inbox(&inbox, authority, &bytes).await;
        }
        let created = (id.parse::<u64>().map_err(|_| GatewayError::Protocol)? >> 22) as i64
            + 1_420_070_400_000;
        let elapsed = (self.store.clock)().saturating_sub(created);
        if elapsed < 0 {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(2500_i64.saturating_sub(elapsed).max(0) as u64);
        let current = super::runtime_source::resolve(
            &self.store.channels,
            &self.store.keys,
            &runtime.ingress.resolver,
            &authority.record,
        )
        .await?;
        if current.revision != authority.revision {
            return Err(GatewayError::Authority);
        }
        let (mut activity, values) = match parse(&event.data, &authority.bot_id, created) {
            Ok(value) => value,
            Err(_) => return Ok(()),
        };
        let Some(Source::Discord { admission, .. }) =
            current.channel.definition.source_of(&activity.source)
        else {
            return Err(GatewayError::Authority);
        };
        if admit(
            admission,
            &AdmissionFacts {
                receiving_bot_id: Some(&authority.bot_id),
                mentioned: Some(true),
                sender_id: &activity.actor.id,
                chat_id: &activity.conversation.id,
                is_group: activity.conversation.kind == ConversationKind::Group,
                is_bot_sender: false,
                is_system: false,
                addressed: Some(true),
            },
        )
        .is_err()
        {
            return Ok(());
        }
        let Some(store) = &runtime.ingress.interactions else {
            return Ok(());
        };
        let data = &event.data["data"];
        let Some(custom) = data["custom_id"].as_str() else {
            return Ok(());
        };
        let message = snowflake(&event.data["message"]["id"])?;
        let (interaction, card) = match store
            .consume_discord(
                &current.channel,
                &activity,
                DiscordClick {
                    id,
                    custom_id: custom,
                    component_type: data["component_type"].as_u64().unwrap_or(0),
                    values: &values,
                    message,
                },
                deadline,
            )
            .await
        {
            Ok(value) => value,
            Err(Error::Denied) => return Ok(()),
            Err(Error::Unavailable) => return Err(GatewayError::Unavailable),
        };
        let occurred_at = card
            .consumed_at()
            .ok_or(GatewayError::Authority)?
            .to_owned();
        let identity = card.discord.ok_or(GatewayError::Authority)?;
        activity.kind = ActivityKind::Interaction(interaction);
        activity.routing = Some(identity.routing.clone());
        activity.occurred_at = occurred_at;
        activity.validate().map_err(|_| GatewayError::Protocol)?;
        let snapshot = Snapshot {
            activity,
            identity,
            message_id: message.into(),
        };
        let mut safe = event.data.clone();
        safe.as_object_mut()
            .ok_or(GatewayError::Protocol)?
            .remove("token");
        let sanitized = super::Dispatch {
            name: event.name.clone(),
            sequence: event.sequence,
            data: safe,
        };
        self.persist_inbox(authority, &sanitized, Some(&snapshot))
            .await?;
        // A create-only claim fences ACK dispatch across lease replacement and
        // uncertain storage responses. A crash after claiming never retries it.
        self.current_authority().await?;
        self.fenced().await?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        let claim = crate::connections::store::index_id(
            "discord-component-ack-v1",
            &[&authority.record, &authority.bot_id, id],
        );
        if self
            .store
            .put(
                &claim,
                &authority.owner,
                &json!({"kind":"discord_component_ack_v1","interaction":id}),
                None,
                Some(created / 1000 + super::journal_model::RETENTION_SECONDS),
            )
            .await
            != Ok(1)
        {
            return Ok(());
        }
        self.fenced().await?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Some(token) = event.data["token"].as_str() {
            if self
                .api
                .acknowledge_component(id, token, remaining)
                .await
                .is_err()
            {
                tracing::warn!("native Discord component acknowledgement unavailable; durable callback retained");
            }
        }
        Ok(())
    }
}

fn parse(raw: &Value, bot: &str, created: i64) -> Result<(Activity, Vec<String>), GatewayError> {
    let id = snowflake(&raw["id"])?;
    let chat = snowflake(&raw["channel_id"])?;
    let message = &raw["message"];
    let group = raw.get("guild_id").is_some();
    if group {
        snowflake(&raw["guild_id"])?;
    }
    let user = if group {
        &raw["member"]["user"]
    } else {
        &raw["user"]
    };
    let actor = snowflake(&user["id"])?;
    if raw["token"].as_str().is_none_or(|token| {
        token.is_empty()
            || token.len() > 2048
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    }) {
        return Err(GatewayError::Protocol);
    }
    if snowflake(&raw["application_id"])? != bot
        || snowflake(&message["author"]["id"])? != bot
        || message["author"]["bot"] != true
        || snowflake(&message["channel_id"])? != chat
        || raw
            .pointer("/channel/id")
            .is_some_and(|value| value.as_str() != Some(chat))
        || matches!(
            raw.pointer("/channel/type").and_then(Value::as_u64),
            Some(10..=12)
        )
        || actor == bot
        || super::facts::bool_fact(user, "bot").map_err(|_| GatewayError::Protocol)?
        || super::facts::bool_fact(user, "system").map_err(|_| GatewayError::Protocol)?
        || !matches!(raw["data"]["component_type"].as_u64(), Some(2 | 3))
    {
        return Err(GatewayError::Authority);
    }
    snowflake(&message["id"])?;
    let values = match raw["data"].get("values") {
        None => vec![],
        Some(value) => value
            .as_array()
            .filter(|v| v.len() <= 1)
            .ok_or(GatewayError::Protocol)?
            .iter()
            .map(|v| {
                v.as_str()
                    .filter(|s| s.len() <= 10)
                    .map(str::to_owned)
                    .ok_or(GatewayError::Protocol)
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    let activity = Activity {
        schema_version: 1,
        event_id: EventId::new(format!("discord:{bot}:interaction:{id}"))
            .map_err(|_| GatewayError::Protocol)?,
        kind: ActivityKind::Interaction(InteractionResult {
            block_id: "pending".into(),
            action_id: "pending".into(),
            values: Default::default(),
        }),
        actor: Actor {
            id: actor.into(),
            display_name: None,
            kind: ActorKind::Human,
        },
        conversation: ActivityConversation {
            id: chat.into(),
            kind: if group {
                ConversationKind::Group
            } else {
                ConversationKind::Private
            },
            thread_id: None,
        },
        occurred_at: time::OffsetDateTime::from_unix_timestamp_nanos(
            i128::from(created) * 1_000_000,
        )
        .map_err(|_| GatewayError::Protocol)?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| GatewayError::Protocol)?,
        source: SourceKind::Discord { bot_id: bot.into() },
        routing: None,
        thread: None,
    };
    Ok((activity, values))
}
