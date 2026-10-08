//! One delivery worker: it runs its share of events' commands in arrival
//! order and drives their timers.
use super::{
    address::{
        count_unavailable, default_ok, effective, handoff_controls, requested, routing_send,
        routing_send_for_message, AddressClass, EffectiveAddress, SendKind,
        DEFAULT_REPLY_UNAVAILABLE,
    },
    event::{key, now},
    failure::{consumer_failed, ConsumerFailure},
    gate::Ungated,
    group::{consumer_of, Groups},
    intents::{DeliveryCommand, DeliveryOutcome, ReplySource},
    label::{self, active_label},
    notice::{send_notice, Notice},
    part_keys::PartKeys,
    send::{unsupported, SendContext, Sender},
    step::{custom_outcome, outcome_event, output_address, reply, timer_step},
};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::launch::LaunchRequest,
    turns::{outcomes::TurnOutcome, TurnTracker},
    DeliveryConfig,
};
use cmaeg_core::{
    channel::ReplyChoice,
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};
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
            destination,
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
                    destination,
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
    destination: Option<ReplyChoice>,
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
        destination,
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
    // The handler resolved this snapshot at the call, so a default
    // destination changed or detached since applies to later calls only.
    let mut address = effective(
        &channel.definition,
        &event,
        requested(destination, ReplyChoice::Origin, &event),
        sender.custom.sync_registered(&event.reply),
        config.multi_consumer_enabled,
        SendKind::Tool,
    );
    if let EffectiveAddress::Rejected { reason } = address {
        return reject_tool_reply(&channel, &event, receipt, reason);
    }
    let id = key(event.channel_id.as_str(), event.event_id.as_str());
    if !groups.reserve(&id, &consumer_of(&event)) {
        return reject_tool_reply(&channel, &event, receipt, "delivery_capacity");
    }
    let mut sync_attempt = None;
    if matches!(address, EffectiveAddress::Origin(ReplyAddress::Sync { .. }))
        && default_ok(
            &channel.definition,
            &event,
            config.multi_consumer_enabled,
            SendKind::Tool,
        )
        && !handoff_controls(&event, &message)
    {
        match sender.custom.begin_reply(&Ungated, &event, &message) {
            Err(result) => {
                let _ = receipt.send(result);
                return;
            }
            Ok(streamed) => {
                sync_attempt = Some(streamed);
                if !streamed {
                    address = effective(
                        &channel.definition,
                        &event,
                        requested(destination, ReplyChoice::Origin, &event),
                        false,
                        config.multi_consumer_enabled,
                        SendKind::Tool,
                    );
                }
            }
        }
    }
    // Generate keys only after a lost Sync waiter actually selects the relay.
    let keys = match &address {
        EffectiveAddress::DefaultRelayConversation { .. } => match PartKeys::tool() {
            Ok(keys) => Some(keys),
            Err(_) => {
                tracing::error!("no random seed for a default relay send's keys");
                return reject_tool_reply(&channel, &event, receipt, DEFAULT_REPLY_UNAVAILABLE);
            }
        },
        _ => None,
    };
    let label = active_label(&channel.definition, event.consumer.as_ref(), routed);
    let (group, at) = groups.join(id, &channel, &event);
    // A tool uses its call-time snapshot only for this send. The serial
    // worker cannot observe the temporary replacement from another command.
    let turn_channel = std::mem::replace(&mut group.channel, channel);
    group.consumers[at].event = event;
    let result = if let (Some(streamed), EffectiveAddress::Origin(_)) = (sync_attempt, &address) {
        complete_sync_reply(
            sender,
            &group.channel,
            &group.consumers[at].event,
            &message,
            streamed,
        )
        .await
    } else {
        reply(
            sender,
            group,
            at,
            &message,
            label.as_deref(),
            &address,
            keys.as_ref(),
        )
        .await
    };
    group.channel = turn_channel;
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

#[tracing::instrument(name="cmaeg.deliver", skip_all, fields(
    channel = %crate::telemetry::channel_hash(channel.id.as_str()), event_id = %event.event_id,
    consumer = event.consumer.as_ref().map_or("default", |id| id.as_str())))]
async fn complete_sync_reply(
    sender: &Sender,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    message: &OutboundMessage,
    streamed: bool,
) -> super::intents::IntentReceipt {
    let mut observation = crate::telemetry::operations::Operation::new(
        crate::telemetry::names::DELIVERY_DURATION,
        crate::telemetry::names::PLATFORM,
        "custom",
    );
    let result = sender
        .custom
        .complete_reply(&Ungated, channel, event, message, streamed)
        .await;
    observation.finish(result.delivery.kind());
    result
}

/// Answer a tool reply that may not be sent with `reason`, logged at warn.
fn reject_tool_reply(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    receipt: oneshot::Sender<super::intents::IntentReceipt>,
    reason: &'static str,
) {
    if reason == DEFAULT_REPLY_UNAVAILABLE {
        count_unavailable(&channel.definition);
    }
    tracing::warn!(
        channel = %crate::telemetry::channel_hash(channel.id.as_str()),
        consumer = %consumer_of(event),
        reason,
        "tool reply rejected"
    );
    let mut result = unsupported();
    result.delivery = DeliveryOutcome::Rejected {
        reason: reason.into(),
    };
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
    let streamed = custom && sender.custom.stream(&opened.payload, &outcome);
    let (group, at) = groups.join(id, &turn.channel, &opened.payload);
    // A newer tool call may have created the group before this older turn's
    // first outcome. Automatic outputs always use the admission snapshot.
    group.set_channel(turn.channel.clone());
    let delivery = &mut group.consumers[at];
    delivery.routed_turn = turn.activity.routing.is_some();
    delivery.tool_replied |= tracker.has_tool_reply(turn.event_ref.as_str(), now());
    if delivery.terminal {
        return;
    }
    let address = output_address(sender, group, at, config.multi_consumer_enabled, streamed);
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
