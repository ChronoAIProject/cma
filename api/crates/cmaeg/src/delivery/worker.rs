//! One delivery worker: it runs its share of events' commands in arrival
//! order and drives their timers.
use super::{
    address::{
        count_unavailable, routing_send, routing_send_for_message, tool_address, AddressClass,
        EffectiveAddress, SendKind, DEFAULT_REPLY_UNAVAILABLE,
    },
    event::{key, now},
    failure::{consumer_failed, ConsumerFailure},
    gate::Ungated,
    group::{consumer_of, Groups},
    intents::{DeliveryCommand, DeliveryOutcome, ReplySource},
    label::{self, active_label},
    notice::{send_notice, Notice},
    send::{unsupported, SendContext, Sender},
    step::{custom_outcome, outcome_event, output_address, reply, timer_step},
};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::launch::LaunchRequest,
    turns::{outcomes::TurnOutcome, TurnTracker},
    DeliveryConfig,
};
use cmaeg_core::{message::OutboundMessage, seal::EventRefPayload};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};
use tracing::Instrument;
pub(super) async fn run(
    mut commands: mpsc::Receiver<DeliveryCommand>,
    sender: Arc<Sender>,
    config: DeliveryConfig,
    tracker: Arc<TurnTracker>,
) {
    // Each event belongs to exactly one worker, so its group requires no cross-worker lock.
    let mut groups = Groups::default();
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => {
                let Some(command) = command else { break; };
                let trace = command.trace();
                handle(command, &mut groups, &sender, &config, &tracker).instrument(trace).await;
            }
            _ = tick.tick(), if !commands.is_closed() => {
                groups.expire();
                timers(&mut groups, &sender, &config).await;
            }
        }
    }
}
async fn handle(
    command: DeliveryCommand,
    groups: &mut Groups,
    sender: &Sender,
    config: &DeliveryConfig,
    tracker: &TurnTracker,
) {
    match command {
        DeliveryCommand::Update {
            trace,
            channel,
            message_ref,
            message,
            receipt,
        } => {
            // An update has no group; its effects touch none either. An
            // edited message keeps its author's label under today's rule.
            let routed = routing_send_for_message(
                &channel.definition,
                &message_ref,
                config.multi_consumer_enabled,
            );
            let label = active_label(&channel.definition, message_ref.consumer.as_ref(), routed);
            let transport = if message_ref.route_id.starts_with("telegram:") {
                "telegram"
            } else {
                "relay"
            };
            label::count(label.as_deref(), Some(transport));
            let context = SendContext {
                trace: &trace,
                channel: &channel,
                first_send_done: false,
                gate: &Ungated,
                label: label.as_deref(),
            };
            let (result, _) = sender.update(&context, &message_ref, &message).await;
            let _ = receipt.send(result);
        }
        DeliveryCommand::Typing {
            channel,
            event,
            receipt,
            ..
        } => {
            let _ = receipt.send(sender.typing(&channel, &event.reply).await);
        }
        DeliveryCommand::Reply {
            channel,
            event,
            message,
            source,
            receipt,
            ..
        } => {
            tool_reply(
                groups,
                sender,
                config,
                ToolReply {
                    channel,
                    event,
                    message,
                    source,
                    receipt,
                },
            )
            .await
        }
        DeliveryCommand::StreamOutcome { turn, outcome } => {
            stream_outcome(groups, sender, config, tracker, turn, outcome).await
        }
        DeliveryCommand::ConsumerFailed {
            channel,
            payload,
            consumer,
            code,
            provider_code,
        } => {
            let failure = ConsumerFailure {
                channel,
                payload,
                consumer,
                code,
                provider_code,
            };
            consumer_failed(groups, sender, config, failure).await
        }
        DeliveryCommand::Notice {
            trace,
            channel,
            address,
            event_id,
            text,
        } => {
            let notice = Notice {
                trace,
                channel,
                address,
                event_id,
                text,
            };
            send_notice(sender, notice).await
        }
    }
}

struct ToolReply {
    channel: Arc<ResolvedChannel>,
    event: EventRefPayload,
    message: OutboundMessage,
    source: ReplySource,
    receipt: oneshot::Sender<super::intents::IntentReceipt>,
}

/// A reply its handler sent for one consumer of an event. It joins that
/// consumer's entry, so its outcome suppresses only that consumer's auto-final.
async fn tool_reply(
    groups: &mut Groups,
    sender: &Sender,
    config: &DeliveryConfig,
    command: ToolReply,
) {
    let ToolReply {
        channel,
        event,
        message,
        source,
        receipt,
    } = command;
    if receipt.is_closed() {
        return;
    }
    let routed = routing_send(
        &channel.definition,
        &event,
        config.multi_consumer_enabled,
        SendKind::Tool,
    );
    if let EffectiveAddress::Rejected { reason } = tool_address(&event, routed) {
        if reason == DEFAULT_REPLY_UNAVAILABLE {
            count_unavailable(&channel.definition);
        }
        tracing::warn!(
            channel = %crate::telemetry::channel_hash(channel.id.as_str()),
            consumer = %consumer_of(&event),
            reason,
            "tool reply rejected"
        );
        let mut result = unsupported();
        result.delivery = DeliveryOutcome::Rejected {
            reason: reason.into(),
        };
        let _ = receipt.send(result);
        return;
    }
    let id = key(event.channel_id.as_str(), event.event_id.as_str());
    if !groups.reserve(&id, &consumer_of(&event)) {
        let mut result = unsupported();
        result.delivery = DeliveryOutcome::Rejected {
            reason: "delivery_capacity".into(),
        };
        let _ = receipt.send(result);
        return;
    }
    let label = active_label(&channel.definition, event.consumer.as_ref(), routed);
    let (group, at) = groups.join(id, &channel, &event);
    group.set_channel(channel);
    group.consumers[at].event = event;
    let result = reply(sender, group, at, &message, label.as_deref()).await;
    if matches!(
        result.delivery,
        DeliveryOutcome::Confirmed { .. } | DeliveryOutcome::Unknown { .. }
    ) {
        let delivery = &mut group.consumers[at];
        delivery.last_output_at = Instant::now();
        if source == ReplySource::Tool {
            delivery.tool_replied = true;
        }
    }
    let _ = receipt.send(result);
}

/// One provider outcome of one consumer's turn. An outcome after that
/// consumer is terminal is ignored for it alone; the event's other consumers
/// carry on.
async fn stream_outcome(
    groups: &mut Groups,
    sender: &Sender,
    config: &DeliveryConfig,
    tracker: &TurnTracker,
    turn: Arc<LaunchRequest>,
    outcome: TurnOutcome,
) {
    tracing::info!(outcome = outcome.kind(), "provider turn outcome");
    // This is an already-admitted internal turn. Tool ingress separately enforces
    // ref expiry; a long turn may still deliver with its retained channel key.
    let Ok(opened) = cmaeg_core::seal::open::<EventRefPayload>(
        &sender.keys,
        cmaeg_core::seal::TokenKind::EventRef,
        turn.event_ref.as_str(),
    ) else {
        return;
    };
    if opened.payload.validate().is_err()
        || opened.payload.channel_id != turn.channel.id
        || opened.payload.event_id != turn.activity.event_id
        || opened.payload.record_id != turn.channel.pointer.record_id
    {
        return;
    }
    let id = key(turn.channel.id.as_str(), turn.activity.event_id.as_str());
    if !groups.reserve(&id, &consumer_of(&opened.payload)) {
        tracing::warn!("delivery event capacity reached");
        return;
    }
    let custom = matches!(
        turn.activity.source,
        cmaeg_core::activity::SourceKind::Custom
    );
    if custom {
        sender.custom.stream(&opened.payload, &outcome);
    }
    let (group, at) = groups.join(id, &turn.channel, &opened.payload);
    let delivery = &mut group.consumers[at];
    delivery.routed_turn = turn.activity.routing.is_some();
    delivery.tool_replied |= tracker.has_tool_reply(turn.event_ref.as_str(), now());
    if delivery.terminal {
        return;
    }
    let address = output_address(group, at, config.multi_consumer_enabled);
    // An unrouted activity keeps today's branch on its source verbatim. A
    // routed one takes its consumer's effective address's branch; a
    // rejected address keeps its origin's, where its outputs are refused.
    let custom = if turn.activity.routing.is_some() {
        let class = address
            .class()
            .unwrap_or_else(|| AddressClass::of(&group.consumers[at].event.reply));
        class == AddressClass::Custom
    } else {
        custom
    };
    if custom {
        custom_outcome(group, at, outcome, sender, &address).await;
        return;
    }
    outcome_event(group, at, outcome, sender, config).await;
}

async fn timers(groups: &mut Groups, sender: &Sender, config: &DeliveryConfig) {
    for group in groups.values_mut() {
        if timer_step(group, sender, config).await {
            return;
        }
    }
}
