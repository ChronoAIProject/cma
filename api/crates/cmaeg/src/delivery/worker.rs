use super::{
    event::{key, normalized_consumer, now, EventKey, EventState},
    gate::{SendGate, Ungated},
    intents::{DeliveryCommand, DeliveryOutcome, ReplySource},
    send::{unsupported, SendContext, Sender},
    step::{finish, outcome_event, reply, timer_step},
};
use crate::{
    turns::{observer::DeliveryStep, outcomes::TurnOutcome, TurnTracker},
    DeliveryConfig,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::Instant;
use tracing::Instrument;
const MAX_EVENTS: usize = 8192;
const EVENT_RETENTION: Duration = Duration::from_secs(30);
pub(super) async fn run(
    mut commands: super::queue::Receiver<DeliveryCommand>,
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
                    reply.answer_response_id.as_deref(),
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
            let result = reply(sender, state, &message, step, None).await;
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
        DeliveryCommand::RetireStream { turn } => {
            if let Some(event) = stream_event(sender, &turn) {
                retire_stream(events, &stream_key(&event), &event);
            }
        }
        DeliveryCommand::StreamOutcome {
            turn,
            outcome,
            answer_response_id,
            accepted_deadline,
            recovery,
        } => {
            tracing::info!(outcome = outcome.kind(), "provider turn outcome");
            let Some(event) = stream_event(sender, &turn) else {
                return;
            };
            let id = stream_key(&event);
            if let Some(authority) = &recovery {
                if authority.current().await.is_none() {
                    retire_stream(events, &id, &event);
                    return;
                }
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
                TurnOutcome::Completed { final_text } => {
                    let consumer = turn.consumer.as_ref().map_or("default", |id| id.as_str());
                    if final_text
                        .as_ref()
                        .is_some_and(|text| !text.trim().is_empty())
                    {
                        sender
                            .observer
                            .checkpoint(
                                &event.record_id,
                                &event.event_id,
                                crate::activity::Change::state(
                                    consumer,
                                    crate::activity::ConsumerState::Produced,
                                    None,
                                ),
                            )
                            .await;
                    }
                    Some(crate::activity::Change::state(
                        consumer,
                        crate::activity::ConsumerState::Completed,
                        None,
                    ))
                }
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
                    .checkpoint(&event.record_id, &event.event_id, change)
                    .await;
            }
            if !reserve(events, &id) {
                tracing::warn!("delivery event capacity reached");
                return;
            }
            let custom = matches!(
                turn.activity.source,
                cmaeg_core::activity::SourceKind::Custom
            );
            let sync_authorized = match &recovery {
                Some(authority)
                    if matches!(&event.reply, cmaeg_core::seal::ReplyAddress::Sync { .. }) =>
                {
                    authority.permits_sync().await
                }
                Some(_) => false,
                None => true,
            };
            if custom && sync_authorized {
                let buffered = sender.custom.stream(&event, &outcome);
                if outcome.terminal() {
                    sender.custom.observe_local(&event, buffered).await;
                }
            }
            let state = events
                .entry(id)
                .or_insert_with(|| EventState::new(turn.channel.clone(), event));
            state.tool_replied |= tracker.has_tool_reply(turn.event_ref.as_str(), now());
            if state.terminal {
                return;
            }
            if custom {
                if matches!(outcome, TurnOutcome::Accepted { .. }) {
                    state.terminal_at = None;
                    state.accepted_deadline = state
                        .accepted_deadline
                        .max(accepted_deadline.map(Instant::from_std));
                }
                if let Some(delivered) = sender
                    .custom
                    .outcome(
                        recovery
                            .as_deref()
                            .map_or(&Ungated as &dyn SendGate, |gate| gate),
                        if recovery.is_some() {
                            &turn.channel
                        } else {
                            &state.channel
                        },
                        &state.event,
                        &outcome,
                        answer_response_id.as_deref(),
                    )
                    .await
                {
                    let step = match outcome {
                        TurnOutcome::Completed { .. } => DeliveryStep::Answer,
                        _ => DeliveryStep::Failure,
                    };
                    sender.observe(&state.event, step, &delivered, None).await;
                }
                if outcome.terminal() {
                    finish(state);
                }
                return;
            }
            outcome_event(
                state,
                outcome,
                answer_response_id.as_deref(),
                sender,
                config,
            )
            .await;
        }
    }
}
fn stream_event(
    sender: &Sender,
    turn: &crate::ingress::launch::LaunchRequest,
) -> Option<cmaeg_core::seal::EventRefPayload> {
    use cmaeg_core::seal::{open, EventRefPayload, TokenKind};

    // A retained turn can outlive the EventRef's separate tool admission window.
    let event = open::<EventRefPayload>(&sender.keys, TokenKind::EventRef, turn.event_ref.as_str())
        .ok()?
        .payload;
    event.validate().ok()?;
    (event.channel_id == turn.channel.id
        && event.event_id == turn.activity.event_id
        && event.record_id == turn.channel.pointer.record_id
        && event.conversation_id == turn.conversation_id
        && normalized_consumer(event.consumer.as_ref())
            == normalized_consumer(turn.consumer.as_ref()))
    .then_some(event)
}

fn stream_key(event: &cmaeg_core::seal::EventRefPayload) -> EventKey {
    key(
        event.channel_id.as_str(),
        event.event_id.as_str(),
        event.consumer.as_ref(),
    )
}

fn retire_stream(
    events: &mut HashMap<EventKey, EventState>,
    id: &EventKey,
    event: &cmaeg_core::seal::EventRefPayload,
) {
    if let Some(state) = events.get_mut(id).filter(|state| {
        !state.terminal
            && state.event.record_id == event.record_id
            && state.event.conversation_id == event.conversation_id
    }) {
        finish(state);
    }
}

fn reserve(events: &mut HashMap<EventKey, EventState>, id: &EventKey) -> bool {
    if events.len() >= MAX_EVENTS {
        expire(events);
    }
    events.contains_key(id) || events.len() < MAX_EVENTS
}
fn expire(events: &mut HashMap<EventKey, EventState>) {
    let now = Instant::now();
    events.retain(|_, state| {
        // Retain a queued terminal outcome through the ordinary retention window,
        // even when a full queue prevented the turn from submitting retirement.
        state
            .terminal_at
            .or(state.accepted_deadline)
            .is_none_or(|at| now.saturating_duration_since(at) < EVENT_RETENTION)
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
