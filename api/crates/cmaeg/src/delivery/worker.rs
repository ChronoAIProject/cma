use super::{
    event::{key, normalized_consumer, now, EventKey, EventState},
    gate::Ungated,
    intents::{DeliveryCommand, DeliveryOutcome, ReplySource},
    send::{unsupported, SendContext, Sender},
    step::{finish, outcome_event, reply, timer_step},
};
use crate::{
    turns::{observer::DeliveryStep, outcomes::TurnOutcome, TurnTracker},
    DeliveryConfig,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{sync::mpsc, time::Instant};
use tracing::Instrument;
const MAX_EVENTS: usize = 8192;
pub(super) async fn run(
    mut commands: mpsc::Receiver<DeliveryCommand>,
    sender: Arc<Sender>,
    config: DeliveryConfig,
    tracker: Arc<TurnTracker>,
) {
    // Each event/consumer belongs to one worker; the fixed state cap includes every consumer.
    let mut events = HashMap::<EventKey, EventState>::new();
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => {
                let Some(command) = command else { break; };
                let trace = command.trace();
                handle(command, &mut events, &sender, &config, &tracker).instrument(trace).await;
            }
            _ = tick.tick(), if !commands.is_closed() => {
                expire(&mut events);
                timers(&mut events, &sender, &config).await;
            }
        }
    }
}
async fn handle(
    command: DeliveryCommand,
    events: &mut HashMap<EventKey, EventState>,
    sender: &Sender,
    config: &DeliveryConfig,
    tracker: &TurnTracker,
) {
    match command {
        DeliveryCommand::DurableReply(reply) => {
            if reply.receipt.is_closed() {
                return;
            }
            let context = SendContext {
                trace: &reply.trace,
                channel: &reply.channel,
                // A replay never consumes a single-use ingress reply token.
                first_send_done: true,
                gate: reply.gate.as_ref(),
            };
            let result = match &reply.default_destination {
                Some((destination, operation)) => {
                    sender
                        .default_reply(
                            &context,
                            &reply.event,
                            &reply.message,
                            destination,
                            operation,
                        )
                        .await
                }
                None => sender.reply(&context, &reply.event, &reply.message).await.0,
            };
            let step = match reply.source {
                ReplySource::Tool => DeliveryStep::ToolReply,
                ReplySource::Stream => DeliveryStep::Answer,
            };
            sender
                .observe_at(
                    &reply.event,
                    step,
                    &result.delivery,
                    if reply.default_destination.is_some() {
                        crate::turns::observer::Destination::Default
                    } else {
                        crate::turns::observer::Destination::Origin
                    },
                    reply
                        .default_destination
                        .as_ref()
                        .map(|(destination, _)| destination),
                )
                .await;
            let _ = reply.receipt.send(result);
        }
        DeliveryCommand::Update {
            trace,
            channel,
            message_ref,
            message,
            receipt,
        } => {
            // An update has no event state; its effects touch none either.
            let context = SendContext {
                trace: &trace,
                channel: &channel,
                first_send_done: false,
                gate: &Ungated,
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
            if receipt.is_closed() {
                return;
            }
            let id = key(
                event.channel_id.as_str(),
                event.event_id.as_str(),
                event.consumer.as_ref(),
            );
            if !reserve(events, &id) {
                let mut result = unsupported();
                result.delivery = DeliveryOutcome::Rejected {
                    reason: "delivery_capacity".into(),
                };
                let _ = receipt.send(result);
                return;
            }
            let state = events
                .entry(id)
                .or_insert_with(|| EventState::new(channel.clone(), event.clone()));
            state.channel = channel;
            state.event = event;
            let step = match source {
                ReplySource::Tool => DeliveryStep::ToolReply,
                ReplySource::Stream => DeliveryStep::Answer,
            };
            let result = reply(sender, state, &message, step).await;
            if matches!(
                result.delivery,
                DeliveryOutcome::Confirmed { .. } | DeliveryOutcome::Unknown { .. }
            ) {
                state.last_output_at = Instant::now();
                if source == ReplySource::Tool {
                    state.tool_replied = true;
                }
            }
            let _ = receipt.send(result);
        }
        DeliveryCommand::StreamOutcome { turn, outcome } => {
            tracing::info!(outcome = outcome.kind(), "provider turn outcome");
            // This is an already-admitted internal turn. Tool ingress separately enforces
            // ref expiry; a long turn may still deliver with its retained channel key.
            let Ok(opened) = cmaeg_core::seal::open::<cmaeg_core::seal::EventRefPayload>(
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
                || opened.payload.conversation_id != turn.conversation_id
                || normalized_consumer(opened.payload.consumer.as_ref())
                    != normalized_consumer(turn.consumer.as_ref())
            {
                return;
            }
            let progress = match &outcome {
                TurnOutcome::Accepted { response_id, .. } => Some(crate::activity::Change::state(
                    turn.consumer.as_ref().map_or("default", |id| id.as_str()),
                    crate::activity::ConsumerState::Accepted,
                    Some(response_id.clone()),
                )),
                TurnOutcome::MessageCommitted { .. } => Some(crate::activity::Change::state(
                    turn.consumer.as_ref().map_or("default", |id| id.as_str()),
                    crate::activity::ConsumerState::Produced,
                    None,
                )),
                TurnOutcome::Completed { .. } => Some(crate::activity::Change::state(
                    turn.consumer.as_ref().map_or("default", |id| id.as_str()),
                    crate::activity::ConsumerState::Completed,
                    None,
                )),
                TurnOutcome::Failed { .. } | TurnOutcome::Incomplete { .. } => {
                    Some(crate::activity::Change::state(
                        turn.consumer.as_ref().map_or("default", |id| id.as_str()),
                        crate::activity::ConsumerState::Failed,
                        None,
                    ))
                }
                _ => None,
            };
            if let Some(change) = progress {
                sender
                    .observer
                    .checkpoint(&opened.payload.record_id, &opened.payload.event_id, change)
                    .await;
            }
            let id = key(
                opened.payload.channel_id.as_str(),
                opened.payload.event_id.as_str(),
                opened.payload.consumer.as_ref(),
            );
            if !reserve(events, &id) {
                tracing::warn!("delivery event capacity reached");
                return;
            }
            let custom = matches!(
                turn.activity.source,
                cmaeg_core::activity::SourceKind::Custom
            );
            if custom {
                let buffered = sender.custom.stream(&opened.payload, &outcome);
                if outcome.terminal() {
                    sender.custom.observe_local(&opened.payload, buffered).await;
                }
            }
            let state = events
                .entry(id)
                .or_insert_with(|| EventState::new(turn.channel.clone(), opened.payload));
            state.tool_replied |= tracker.has_tool_reply(turn.event_ref.as_str(), now());
            if state.terminal {
                return;
            }
            if custom {
                if matches!(outcome, TurnOutcome::Accepted { .. }) {
                    state.terminal_at = None;
                }
                if let Some(delivered) = sender
                    .custom
                    .outcome(&state.channel, &state.event, &outcome)
                    .await
                {
                    let step = match outcome {
                        TurnOutcome::Completed { .. } => DeliveryStep::Answer,
                        _ => DeliveryStep::Failure,
                    };
                    sender.observe(&state.event, step, &delivered).await;
                }
                if outcome.terminal() {
                    finish(state);
                }
                return;
            }
            outcome_event(state, outcome, sender, config).await;
        }
    }
}
fn reserve(events: &mut HashMap<EventKey, EventState>, id: &EventKey) -> bool {
    if events.len() >= MAX_EVENTS {
        expire(events);
    }
    events.contains_key(id) || events.len() < MAX_EVENTS
}
fn expire(events: &mut HashMap<EventKey, EventState>) {
    events.retain(|_, state| {
        state
            .terminal_at
            .is_none_or(|at| at.elapsed() < Duration::from_secs(30))
    });
}
async fn timers(
    events: &mut HashMap<EventKey, EventState>,
    sender: &Sender,
    config: &DeliveryConfig,
) {
    for state in events.values_mut() {
        if timer_step(state, sender, config).await {
            return;
        }
    }
}
