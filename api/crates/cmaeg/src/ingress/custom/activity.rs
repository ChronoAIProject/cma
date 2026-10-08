use crate::{channels::resolver::ResolvedChannel, ingress::launch::LaunchRequest};
use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, Attachment,
        ConversationKind, SourceKind,
    },
    channel::PartitionKey,
    cloudevent::CloudEvent,
    ids::{ConversationId, EventId, IdempotencyKey},
    seal::{EventRef, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Instant};
/// Normalize an authenticated event and the attachments already extracted
/// from it into a sealed, launchable turn.
pub fn launch_request(
    channel: Arc<ResolvedChannel>,
    ce: CloudEvent,
    attachments: Vec<Attachment>,
    reply: ReplyAddress,
    keys: &MasterKeyRing,
    received_at: Instant,
    now: i64,
) -> Result<LaunchRequest, &'static str> {
    let conversation = match channel.definition.partition.key {
        PartitionKey::EventSubject => ce.subject.clone().unwrap_or_else(|| ce.source.clone()),
        PartitionKey::PerEvent => ce.id.clone(),
        PartitionKey::Fixed => "fixed".into(),
        _ => return Err("custom_partition_invalid"),
    };
    let occurred_at = ce.time.clone().unwrap_or_else(|| {
        time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    });
    let event_id = EventId::new(ce.id.clone()).map_err(|_| "event_id_invalid")?;
    let activity = Activity {
        schema_version: 1,
        event_id: event_id.clone(),
        actor: Actor {
            id: ce.source.clone(),
            display_name: None,
            kind: ActorKind::System,
        },
        conversation: ActivityConversation {
            id: conversation,
            kind: ConversationKind::Custom,
            thread_id: None,
        },
        occurred_at,
        source: SourceKind::Custom,
        kind: ActivityKind::Event { ce, attachments },
    };
    activity.validate().map_err(|_| "activity_invalid")?;
    let partition = activity.partition_key(&channel.definition.partition);
    let conversation_id = ConversationId::derive(
        keys,
        &channel.id,
        channel.definition.partition.version,
        &partition,
    );
    let event_ref = EventRef::seal(
        &EventRefPayload {
            v: 1,
            activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
            channel_id: channel.id.clone(),
            record_id: channel.pointer.record_id.clone(),
            event_id: event_id.clone(),
            // Long URI subjects stay in the provider's activity; the compact ref carries their digest.
            partition_key: if partition.len() <= 512 {
                partition
            } else {
                format!("sha256:{:x}", Sha256::digest(partition.as_bytes()))
            },
            conversation_id: conversation_id.clone(),
            expires_at: now.saturating_add(1800),
            reply,
        },
        keys,
    )
    .map_err(|_| "event_ref_too_large")?;
    Ok(LaunchRequest {
        timing: None,
        consumer: None,
        trace: tracing::Span::current(),
        channel,
        activity,
        conversation_id,
        event_ref,
        idempotency_key: IdempotencyKey::from_event(&event_id),
        received_at,
    })
}
