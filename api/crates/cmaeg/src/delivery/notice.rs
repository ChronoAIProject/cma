//! Best-effort routing notices: one message to the origin chat of an event
//! routing refused on a routed channel.
//!
//! A notice is never worth delaying ingress or delivery for: it is queued
//! only if the inbound queue has room, sent at most once, and dropped on any
//! failure. It creates no group and takes no capacity entry.
use super::{
    address::AddressClass,
    gate::Ungated,
    intents::{DeliveryCommand, DeliveryOutcome},
    router::{DeliverySink, Refused},
    send::{SendContext, Sender},
};
use crate::{
    channels::resolver::ResolvedChannel,
    telemetry::{channel_hash, names},
};
use cmaeg_core::{
    ids::{ConversationId, EventId},
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};
use std::sync::Arc;

/// Queue a notice to `address`, the inbound reply address of event
/// `event_id`, without waiting. Returns whether it was queued; a refused
/// notice is dropped and counted in `cmaeg_routing_notices_dropped_total`.
pub fn submit_notice(
    sink: &dyn DeliverySink,
    trace: tracing::Span,
    channel: Arc<ResolvedChannel>,
    address: ReplyAddress,
    event_id: EventId,
    text: String,
) -> bool {
    let command = DeliveryCommand::Notice {
        trace,
        channel,
        address,
        event_id,
        text,
    };
    let Err(refusal) = sink.try_submit(command) else {
        return true;
    };
    let reason = match refusal.reason {
        Refused::Capacity => "queue_full",
        Refused::Closed => "closed",
    };
    if let DeliveryCommand::Notice { channel, .. } = *refusal.command {
        tracing::info!(
            channel = %channel_hash(channel.id.as_str()),
            reason,
            "routing notice dropped"
        );
    }
    metrics::counter!(names::ROUTING_NOTICES_DROPPED, names::REASON => reason).increment(1);
    false
}

/// The fields of a `DeliveryCommand::Notice`.
pub(super) struct Notice {
    pub trace: tracing::Span,
    pub channel: Arc<ResolvedChannel>,
    pub address: ReplyAddress,
    pub event_id: EventId,
    pub text: String,
}

/// Send `notice` once through today's reply path: paced on its route, with
/// the inbound reply token on first use and the channel key after.
pub(super) async fn send_notice(sender: &Sender, notice: Notice) {
    let Notice {
        trace,
        channel,
        address,
        event_id,
        text,
    } = notice;
    let channel_hash = channel_hash(channel.id.as_str());
    if AddressClass::of(&address) != AddressClass::Chat {
        tracing::info!(channel = %channel_hash, "routing notice skipped: its origin is not a chat");
        return;
    }
    // A refused event has no sealed reference, so the notice addresses its
    // origin through an unsealed one. Its identity fields only name the
    // send: nothing derived from them is returned to anyone.
    let Ok(conversation_id) = ConversationId::new(event_id.as_str()) else {
        tracing::info!(channel = %channel_hash, "routing notice skipped: invalid event id");
        return;
    };
    let event = EventRefPayload {
        v: 1,
        activity_sha256: None,
        channel_id: channel.id.clone(),
        record_id: channel.pointer.record_id.clone(),
        partition_key: event_id.as_str().into(),
        event_id,
        conversation_id,
        expires_at: i64::MAX,
        reply: address,
        consumer: None,
        lineage: None,
    };
    let context = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &Ungated,
        label: None,
    };
    let message = OutboundMessage {
        text: Some(text),
        blocks: vec![],
        reply_to: None,
    };
    let (result, _) = sender.reply(&context, &event, &message).await;
    match result.delivery {
        DeliveryOutcome::Confirmed { .. } => {
            tracing::info!(channel = %channel_hash, "routing notice sent")
        }
        other => tracing::info!(
            channel = %channel_hash,
            outcome = other.kind(),
            "routing notice not delivered; dropped"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::router::ShardedScheduler;
    use cmaeg_core::{channel::ChannelDefinition, seal::ChannelPointer};
    use metrics_exporter_prometheus::PrometheusBuilder;
    use tokio::sync::mpsc;

    fn channel() -> Arc<ResolvedChannel> {
        let definition: ChannelDefinition = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        Arc::new(ResolvedChannel {
            id: cmaeg_core::ids::ChannelId::new("cmaeg1.ch.notice-fixture").unwrap(),
            pointer: ChannelPointer {
                v: 1,
                record_id: definition.record_id.clone(),
                record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
            },
            definition,
            agent_key_hash: [0; 32],
            consumer_keys: Vec::new(),
            loaded_at: std::time::Instant::now(),
            version: 1,
        })
    }

    fn notify(sink: &dyn DeliverySink) -> bool {
        submit_notice(
            sink,
            tracing::Span::none(),
            channel(),
            ReplyAddress::None,
            "msg-1".parse().unwrap(),
            "Nobody here answers that.".into(),
        )
    }

    #[test]
    fn a_notice_refused_by_the_inbound_queue_is_dropped_and_counted() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let metrics = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let (inbound, mut queued) = mpsc::channel(1);
            let sink = ShardedScheduler::new(inbound);
            assert!(notify(&sink), "a queue with room takes it");
            assert!(!notify(&sink), "a full queue refuses it");
            assert!(matches!(
                queued.try_recv(),
                Ok(DeliveryCommand::Notice { .. })
            ));
            assert!(queued.try_recv().is_err(), "the refused notice is dropped");
            drop(queued);
            assert!(!notify(&sink), "a closed queue refuses it");
        });
        let rendered = metrics.render();
        assert!(
            rendered.contains("cmaeg_routing_notices_dropped_total{reason=\"queue_full\"} 1"),
            "{rendered}"
        );
        assert!(
            rendered.contains("cmaeg_routing_notices_dropped_total{reason=\"closed\"} 1"),
            "{rendered}"
        );
    }
}
