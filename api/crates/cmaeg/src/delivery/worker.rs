use super::{
    event::{key, now, EventKey, EventState},
    intents::{DeliveryCommand, DeliveryOutcome, ReplySource},
    send::{unsupported, Sender},
    step::{finish, outcome_event, timer_step},
};
use crate::{
    turns::{outcomes::TurnOutcome, TurnTracker},
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
