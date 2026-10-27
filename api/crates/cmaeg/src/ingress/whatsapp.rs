//! Native message and reaction activities retain unknown account identity.
use super::launch::LaunchRequest;
use crate::{
    channels::resolver::ResolvedChannel,
    whatsapp::webhook::{self, Content, Message},
};
use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, ConversationKind,
        SourceKind,
    },
    channel::Source,
    cloudevent::CloudEvent,
    ids::{ConversationId, IdempotencyKey},
    seal::{event_ref::activity_digest, EventRef, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde_json::json;
use std::{sync::Arc, time::Instant};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub(crate) struct VerifiedWhatsappMessage {
    pub channel: Arc<ResolvedChannel>,
    pub source: Source,
    pub message: Message,
    pub body: Vec<u8>,
    pub received: Instant,
    pub received_at: i64,
}
pub(crate) fn source_kind(source: &Source) -> Option<SourceKind> {
    let Source::Whatsapp {
        app_id,
        waba_id,
        phone_number_id,
        ..
    } = source
    else {
        return None;
    };
    Some(SourceKind::Whatsapp {
        app_id: app_id.clone(),
        waba_id: waba_id.clone(),
        phone_number_id: phone_number_id.clone(),
    })
}

pub(crate) fn launch(
    verified: &VerifiedWhatsappMessage,
    original: Option<(&str, &Actor)>,
    keys: &MasterKeyRing,
    now: i64,
) -> Result<LaunchRequest, ()> {
    let message = &verified.message;
    let Source::Whatsapp {
        app_id,
        waba_id,
        phone_number_id,
        ..
    } = &verified.source
    else {
        return Err(());
    };
    let event_id = webhook::event_id(&verified.source, &message.id).map_err(|_| ())?;
    let occurred_at = OffsetDateTime::from_unix_timestamp(message.timestamp)
        .map_err(|_| ())?
        .format(&Rfc3339)
        .map_err(|_| ())?;
    let kind = match &message.content {
        Content::Text { text } => ActivityKind::Message {
            text: text.clone(),
            attachments: Vec::new(),
            mentions_bot: false,
            reply_to: None,
            album_id: None,
        },
        Content::Reaction { target, emoji } => {
            let (original_event, author) = original.ok_or(())?;
            let reactions = if emoji.is_empty() {
                Vec::new()
            } else {
                vec![json!({"type":"emoji","emoji":emoji})]
            };
            ActivityKind::Event {
                attachments: Vec::new(),
                ce: CloudEvent {
                    specversion: "1.0".into(),
                    id: event_id.to_string(),
                    source: format!("urn:cma:whatsapp:{app_id}:{waba_id}:{phone_number_id}"),
                    r#type: "cma.message.reaction.updated.v1".into(),
                    time: Some(occurred_at.clone()),
                    subject: Some(target.clone()),
                    datacontenttype: Some("application/json".into()),
                    dataschema: None,
                    extensions: Default::default(),
                    data: Some(
                        json!({"message_id":target,"original_event_id":original_event,"original_author":author,
                    "new_reactions":reactions,"provider_timestamp":message.timestamp}),
                    ),
                },
            }
        }
    };
    let activity = Activity {
        schema_version: 1,
        event_id: event_id.clone(),
        kind,
        actor: Actor {
            id: message.sender.clone(),
            display_name: None,
            kind: ActorKind::Unknown,
        },
        conversation: ActivityConversation {
            id: message.sender.clone(),
            kind: ConversationKind::Private,
            thread_id: None,
        },
        source: source_kind(&verified.source).ok_or(())?,
        occurred_at,
        routing: None,
        thread: None,
    };
    activity.validate().map_err(|_| ())?;
    let partition_key = activity.partition_key(&verified.channel.definition.partition);
    let conversation_id = ConversationId::derive(
        keys,
        &verified.channel.id,
        verified.channel.definition.partition.version,
        &partition_key,
    );
    let reply_target = match &message.content {
        Content::Text { .. } => &message.id,
        Content::Reaction { target, .. } => target,
    };
    let payload = EventRefPayload {
        telegram_admission: None,
        v: 1,
        activity_sha256: Some(activity_digest(&activity)),
        channel_id: verified.channel.id.clone(),
        record_id: verified.channel.definition.record_id.clone(),
        event_id: event_id.clone(),
        partition_key,
        conversation_id: conversation_id.clone(),
        expires_at: now + 1800,
        reply: ReplyAddress::Whatsapp {
            app_id: app_id.clone(),
            waba_id: waba_id.clone(),
            phone_number_id: phone_number_id.clone(),
            recipient_id: message.sender.clone(),
            message_id: reply_target.clone(),
        },
        consumer: None,
        lineage: None,
    };
    payload.validate().map_err(|_| ())?;
    Ok(LaunchRequest {
        channel: verified.channel.clone(),
        activity,
        conversation_id,
        event_ref: EventRef::seal(&payload, keys).map_err(|_| ())?,
        idempotency_key: IdempotencyKey::from_event(&event_id),
        received_at: verified.received,
        trace: tracing::Span::current(),
        timing: None,
        consumer: None,
    })
}
