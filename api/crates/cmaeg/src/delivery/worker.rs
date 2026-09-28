use super::{
    event::{key, now, EventKey, EventState},
    intents::{DeliveryCommand, DeliveryOutcome, ReplySource, TypingOutcome, TypingReceipt},
    send::{unsupported, Sender},
};
use crate::{
    turns::{outcomes::TurnOutcome, TurnTracker},
    DeliveryConfig,
};
use cmaeg_core::message::OutboundMessage;
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
    // Each event belongs to exactly one worker, so state requires no cross-worker lock.
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
        DeliveryCommand::Update {
            channel,
            message_ref,
            message,
            receipt,
            ..
        } => {
            let _ = receipt.send(sender.update(&channel, &message_ref, &message).await);
        }
        DeliveryCommand::Typing { receipt, .. } => {
            let _ = receipt.send(TypingReceipt {
                outcome: TypingOutcome::Unsupported,
            });
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
            let id = key(event.channel_id.as_str(), event.event_id.as_str());
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
            let result = sender.reply(state, &message).await;
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
            {
                return;
            }
            let id = key(turn.channel.id.as_str(), turn.activity.event_id.as_str());
            if !reserve(events, &id) {
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
                sender
                    .custom
                    .outcome(&state.channel, &state.event, &outcome)
                    .await;
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
async fn outcome_event(
    state: &mut EventState,
    outcome: TurnOutcome,
    sender: &Sender,
    config: &DeliveryConfig,
) {
    match outcome {
        TurnOutcome::Accepted { .. } => {
            state.terminal_at = None;
            if state.ack_due.is_none()
                && !state.first_send_done
                && state.channel.definition.reply.ack_after_ms > 0
            {
                state.ack_due = Some(
                    Instant::now()
                        + Duration::from_millis(state.channel.definition.reply.ack_after_ms.into()),
                );
            }
        }
        TurnOutcome::TextDelta { .. } => {
            state.last_output_at = Instant::now();
        }
        TurnOutcome::MessageCommitted { text } => {
            state.last_output_at = Instant::now();
            if state.channel.definition.reply.auto_final && !state.tool_replied {
                state.answer_attempted = true;
                send_text(sender, state, text).await;
            }
        }
        TurnOutcome::Completed { final_text } => {
            if state.channel.definition.reply.auto_final
                && !state.answer_attempted
                && !state.tool_replied
            {
                if let Some(text) = final_text {
                    state.answer_attempted = true;
                    send_text(sender, state, text).await;
                }
            }
            finish(state);
        }
        TurnOutcome::Failed { .. } | TurnOutcome::Incomplete { .. } => {
            if !state.tool_replied {
                send_text(sender, state, config.failure_text.clone()).await;
            }
            finish(state);
        }
        TurnOutcome::ToolStarted { .. } | TurnOutcome::ToolFinished { .. } => {}
    }
}
fn finish(state: &mut EventState) {
    state.terminal = true;
    state.terminal_at = Some(Instant::now());
    state.ack_due = None;
}
async fn timers(
    events: &mut HashMap<EventKey, EventState>,
    sender: &Sender,
    config: &DeliveryConfig,
) {
    for state in events.values_mut() {
        if state.terminal_at.is_some() {
            continue;
        }
        if state.ack_due.is_some_and(|at| at <= Instant::now()) {
            state.ack_due = None;
            if !state.first_send_done {
                state.ack_sent = send_text(sender, state, config.ack_text.clone()).await;
                state.last_progress_at = Instant::now();
                return;
            }
        }
        if state.ack_sent
            && !state.tool_replied
            && !state.answer_attempted
            && state.heartbeats < 5
            && state.last_output_at.max(state.last_progress_at).elapsed() >= config.heartbeat
        {
            state.heartbeats += 1;
            send_text(sender, state, "Still working…".into()).await;
            state.last_progress_at = Instant::now();
            return;
        }
    }
}
async fn send_text(sender: &Sender, state: &mut EventState, text: String) -> bool {
    send_message(
        sender,
        state,
        OutboundMessage {
            text: Some(text),
            blocks: vec![],
            reply_to: None,
        },
    )
    .await
}
async fn send_message(sender: &Sender, state: &mut EventState, message: OutboundMessage) -> bool {
    let result = sender.reply(state, &message).await;
    matches!(result.delivery, DeliveryOutcome::Confirmed { .. })
}
