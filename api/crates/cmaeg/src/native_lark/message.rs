//! Bounded authenticated facts; no edit or complete reaction-state inference.
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::{
        admission::{admit, AdmissionFacts},
        launch::LaunchRequest,
    },
};
use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, ConversationKind,
        SourceKind,
    },
    channel::{native_lark::safe_id, NativeLarkIdentity, NativeLarkReply, NativeLarkSource},
    cloudevent::CloudEvent,
    ids::{ConversationId, EventId, IdempotencyKey},
    seal::{EventRef, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Instant};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Message {
    Text {
        #[serde(skip)]
        quoted_message: Option<cmaeg_core::activity::QuotedMessage>,
        message_id: String,
        chat_id: String,
        sender: String,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        resources: Vec<super::content::Resource>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        received_sticker: Option<super::stickers::ReceivedSticker>,
        group: bool,
        thread_id: Option<String>,
        mentioned: bool,
        created_ms: i64,
    },
    Recall {
        event_id: String,
        message_id: String,
        chat_id: String,
        role: String,
        occurred_ms: i64,
    },
    Reaction {
        event_id: String,
        message_id: String,
        sender: String,
        emoji: String,
        added: bool,
        occurred_ms: i64,
    },
}
fn id(value: &Value, prefix: &str) -> Result<String, ()> {
    value
        .as_str()
        .filter(|s| safe_id(s) && s.starts_with(prefix))
        .map(str::to_owned)
        .ok_or(())
}
fn millis(value: &Value, now: i64) -> Result<i64, ()> {
    let s = value.as_str().ok_or(())?;
    let v = s.parse::<i64>().map_err(|_| ())?;
    if v.to_string() != s || v <= 0 || v / 1000 > now + 300 || v / 1000 <= now - 7 * 86400 {
        return Err(());
    }
    Ok(v)
}
pub(crate) fn extract(
    source: &NativeLarkSource,
    raw: &Value,
    now: i64,
) -> Result<Option<Message>, ()> {
    let header = &raw["header"];
    let event = &raw["event"];
    let event_id = id(&header["event_id"], "")?;
    let result = match header["event_type"].as_str() {
        Some("im.message.receive_v1") => {
            let m = &event["message"];
            let sender = &event["sender"];
            if sender["sender_type"] != "user" {
                return Ok(None);
            }
            // A cross-tenant sender is outside this initial single-tenant source.
            if sender["tenant_key"].as_str() != Some(&source.identity.tenant_key) {
                return Err(());
            }
            let chat_id = id(&m["chat_id"], "oc_")?;
            let message_id = id(&m["message_id"], "om_")?;
            let sender = id(&sender["sender_id"]["open_id"], "ou_")?;
            if sender == source.identity.bot_open_id {
                return Ok(None);
            }
            let group = match m["chat_type"].as_str() {
                Some("p2p") => false,
                Some("group") => true,
                _ => return Ok(None),
            };
            let thread_id = match m
                .get("thread_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                Some(s) => Some(id(&Value::String(s.into()), "omt_")?),
                None => None,
            };
            if !group && thread_id.is_some() {
                return Err(());
            }
            let content: Value = serde_json::from_str(
                m["content"]
                    .as_str()
                    .filter(|s| s.len() <= 48 * 1024)
                    .ok_or(())?,
            )
            .map_err(|_| ())?;
            let message_type = m["message_type"].as_str().ok_or(())?;
            let received_sticker = if message_type == "sticker" {
                Some(super::stickers::ReceivedSticker::parse(&content)?)
            } else {
                None
            };
            let (text, resources) = if received_sticker.is_some() {
                (super::stickers::DESCRIPTION.into(), Vec::new())
            } else {
                let Some(parsed) = super::content::parse(message_type, &content)? else {
                    return Ok(None);
                };
                parsed
            };
            let mentions = match m.get("mentions") {
                Some(v) => v
                    .as_array()
                    .filter(|v| v.len() <= 128)
                    .ok_or(())?
                    .as_slice(),
                None => &[],
            };
            let mentioned = mentions.iter().any(|v| {
                v["id"]["open_id"].as_str() == Some(&source.identity.bot_open_id)
                    && v["tenant_key"].as_str() == Some(&source.identity.tenant_key)
                    && v["key"]
                        .as_str()
                        .is_some_and(|k| !k.is_empty() && text.contains(k))
            });
            Message::Text {
                quoted_message: crate::ingress::quoted_context::lark(raw),
                message_id,
                chat_id,
                sender,
                text,
                resources,
                received_sticker,
                group,
                thread_id,
                mentioned,
                created_ms: millis(&m["create_time"], now)?,
            }
        }
        Some("im.message.recalled_v1") => {
            let role = event["recall_type"]
                .as_str()
                .filter(|s| {
                    matches!(
                        *s,
                        "message_owner" | "group_owner" | "group_manager" | "enterprise_manager"
                    )
                })
                .ok_or(())?
                .to_owned();
            Message::Recall {
                event_id,
                message_id: id(&event["message_id"], "om_")?,
                chat_id: id(&event["chat_id"], "oc_")?,
                role,
                occurred_ms: millis(&event["recall_time"], now)?,
            }
        }
        Some(kind @ ("im.message.reaction.created_v1" | "im.message.reaction.deleted_v1")) => {
            if event["operator_type"] != "user" {
                return Ok(None);
            }
            let sender = id(&event["user_id"]["open_id"], "ou_")?;
            if sender == source.identity.bot_open_id {
                return Ok(None);
            }
            let emoji = id(&event["reaction_type"]["emoji_type"], "")?;
            Message::Reaction {
                event_id,
                message_id: id(&event["message_id"], "om_")?,
                sender,
                emoji,
                added: kind.ends_with("created_v1"),
                occurred_ms: millis(&event["action_time"], now)?,
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}
impl Message {
    pub(crate) fn message_id(&self) -> &str {
        match self {
            Self::Text { message_id, .. }
            | Self::Recall { message_id, .. }
            | Self::Reaction { message_id, .. } => message_id,
        }
    }
    pub(crate) fn occurred_ms(&self) -> i64 {
        match self {
            Self::Text { created_ms, .. } => *created_ms,
            Self::Recall { occurred_ms, .. } | Self::Reaction { occurred_ms, .. } => *occurred_ms,
        }
    }
    pub(crate) fn has_sticker(&self) -> bool {
        matches!(
            self,
            Self::Text {
                received_sticker: Some(_),
                ..
            }
        )
    }
    pub(crate) fn has_resources(&self) -> bool {
        matches!(self, Self::Text { resources, .. } if !resources.is_empty())
    }
    pub(crate) fn ordinary(&self) -> bool {
        matches!(self, Self::Text { .. })
    }
    pub(crate) fn event_id(&self, identity: &NativeLarkIdentity) -> Result<EventId, ()> {
        let key = match self {
            Self::Text { message_id, .. } => return original_id(identity, message_id),
            Self::Recall { event_id, .. } | Self::Reaction { event_id, .. } => event_id,
        };
        digest_id("event", identity, key)
    }
}
fn digest_id(kind: &str, identity: &NativeLarkIdentity, id: &str) -> Result<EventId, ()> {
    let bytes = serde_json::to_vec(&(kind, identity, id)).map_err(|_| ())?;
    EventId::new(format!("native-lark:{:x}", Sha256::digest(bytes))).map_err(|_| ())
}
pub(crate) fn original_id(identity: &NativeLarkIdentity, id: &str) -> Result<EventId, ()> {
    digest_id("message", identity, id)
}
pub(crate) fn admission(
    source: &NativeLarkSource,
    actor: &str,
    conversation: &ActivityConversation,
    mentioned: bool,
) -> Result<(), ()> {
    admit(
        &source.admission,
        &AdmissionFacts {
            receiving_bot_id: Some(&source.identity.bot_open_id),
            mentioned: Some(mentioned),
            sender_id: actor,
            chat_id: &conversation.id,
            is_group: conversation.kind != ConversationKind::Private,
            is_bot_sender: false,
            is_system: false,
            addressed: Some(mentioned),
        },
    )
    .map_err(|_| ())
}
pub(crate) struct Original<'a> {
    pub activity: &'a Activity,
    pub original_event: &'a str,
    pub reply: &'a NativeLarkReply,
    pub author: Actor,
}
pub(crate) fn launch(
    channel: Arc<ResolvedChannel>,
    source: &NativeLarkSource,
    message: &Message,
    original: Option<Original<'_>>,
    keys: &MasterKeyRing,
    received: Instant,
    now: i64,
) -> Result<LaunchRequest, ()> {
    let event_id = message.event_id(&source.identity)?;
    let occurred_at =
        time::OffsetDateTime::from_unix_timestamp_nanos(message.occurred_ms() as i128 * 1_000_000)
            .map_err(|_| ())?
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| ())?;
    let credentials = channel.definition.secrets.native_lark.as_ref().ok_or(())?;
    let revision = super::revision(source, credentials);
    let (kind, actor, conversation, reply) = match message {
        Message::Text {
            message_id,
            chat_id,
            sender,
            text,
            resources,
            received_sticker,
            group,
            thread_id,
            mentioned,
            ..
        } => {
            let conversation = ActivityConversation {
                id: chat_id.clone(),
                kind: if *group {
                    ConversationKind::Group
                } else {
                    ConversationKind::Private
                },
                thread_id: thread_id.clone(),
            };
            admission(source, sender, &conversation, *mentioned)?;
            (
                ActivityKind::Message {
                    text: text.clone(),
                    attachments: received_sticker.as_ref().map_or_else(
                        || super::content::attachments(resources, message_id, &revision),
                        |sticker| vec![sticker.attachment(message_id)],
                    ),
                    mentions_bot: *mentioned,
                    reply_to: None,
                    quoted_message: None,
                    album_id: None,
                },
                Actor {
                    id: sender.clone(),
                    display_name: None,
                    kind: ActorKind::Human,
                },
                conversation,
                NativeLarkReply {
                    mentioned: *mentioned,
                    identity: source.identity.clone(),
                    chat_id: chat_id.clone(),
                    message_id: message_id.clone(),
                    thread_id: thread_id.clone(),
                    revision,
                },
            )
        }
        _ => {
            let original = original.ok_or(())?;
            if original.reply.revision != revision || original.reply.identity != source.identity {
                return Err(());
            }
            let mentioned = original.reply.mentioned;
            if original.author.kind == ActorKind::Human {
                admission(
                    source,
                    &original.author.id,
                    &original.activity.conversation,
                    mentioned,
                )?;
            }
            let mut data = json!({"message_id":message.message_id(),"original_event_id":original.original_event,"original_author":original.author});
            let (kind, actor) = match message {
                Message::Recall { chat_id, role, .. } => {
                    if chat_id != &original.reply.chat_id {
                        return Err(());
                    }
                    data["recall_type"] = role.clone().into();
                    (
                        "cma.message.deleted.v1",
                        Actor {
                            id: "unknown".into(),
                            kind: ActorKind::Unknown,
                            display_name: None,
                        },
                    )
                }
                Message::Reaction {
                    sender,
                    emoji,
                    added,
                    ..
                } => {
                    admission(source, sender, &original.activity.conversation, mentioned)?;
                    data["operation"] = if *added { "add" } else { "remove" }.into();
                    data["reaction"] = json!({"type":"emoji","emoji_type":emoji});
                    (
                        "cma.message.reaction.updated.v1",
                        Actor {
                            id: sender.clone(),
                            kind: ActorKind::Human,
                            display_name: None,
                        },
                    )
                }
                _ => return Err(()),
            };
            (
                ActivityKind::Event {
                    ce: CloudEvent {
                        specversion: "1.0".into(),
                        id: event_id.to_string(),
                        source: format!(
                            "native-lark:{}:{}",
                            source.identity.region.as_str(),
                            source.identity.app_id
                        ),
                        r#type: kind.into(),
                        time: Some(occurred_at.clone()),
                        subject: Some(message.message_id().into()),
                        datacontenttype: Some("application/json".into()),
                        dataschema: None,
                        data: Some(data),
                        extensions: Default::default(),
                    },
                    attachments: vec![],
                },
                actor,
                original.activity.conversation.clone(),
                original.reply.clone(),
            )
        }
    };
    if channel.definition.partition.key == cmaeg_core::channel::PartitionKey::Thread
        && conversation.thread_id.is_none()
    {
        return Err(());
    }
    let activity = Activity {
        schema_version: 1,
        thread: None,
        event_id: event_id.clone(),
        kind,
        actor,
        conversation,
        occurred_at,
        source: SourceKind::NativeLark(source.identity.clone()),
        routing: None,
    };
    activity.validate().map_err(|_| ())?;
    let partition_key = activity.partition_key(&channel.definition.partition);
    let conversation_id = ConversationId::derive(
        keys,
        &channel.id,
        channel.definition.partition.version,
        &partition_key,
    );
    let event_ref = EventRef::seal(
        &EventRefPayload {
            v: 1,
            activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
            channel_id: channel.id.clone(),
            record_id: channel.definition.record_id.clone(),
            event_id: event_id.clone(),
            partition_key,
            conversation_id: conversation_id.clone(),
            expires_at: now + 1800,
            reply: ReplyAddress::NativeLark(reply),
            consumer: None,
            lineage: None,
            telegram_admission: None,
            turn_trace: None,
        },
        keys,
    )
    .map_err(|_| ())?;
    Ok(LaunchRequest {
        timing: None,
        consumer: None,
        trace: tracing::Span::current(),
        channel,
        activity,
        conversation_id,
        event_ref,
        idempotency_key: IdempotencyKey::from_event(&event_id),
        received_at: received,
    })
}
