//! Per-event/consumer delivery steps, shared by every scheduler that delivers an event.
use super::{
    event::EventState,
    gate::Ungated,
    intents::{DeliveryOutcome, IntentReceipt},
    send::{SendContext, Sender},
};
use crate::{
    turns::{observer::DeliveryStep, outcomes::TurnOutcome},
    DeliveryConfig,
};
use cmaeg_core::message::OutboundMessage;
use std::time::Duration;
use tokio::time::Instant;
pub(super) async fn outcome_event(
    state: &mut EventState,
    outcome: TurnOutcome,
    sender: &Sender,
    config: &DeliveryConfig,
) {
    match outcome {
        TurnOutcome::Accepted { thread_reply, .. } => {
            state.thread_reply = thread_reply;
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
                send_text(sender, state, text, DeliveryStep::Answer).await;
            }
        }
        TurnOutcome::Completed { final_text } => {
            if state.channel.definition.reply.auto_final
                && !state.answer_attempted
                && !state.tool_replied
            {
                if let Some(text) = final_text {
                    state.answer_attempted = true;
                    send_text(sender, state, text, DeliveryStep::Answer).await;
                }
            }
            finish(state);
        }
        TurnOutcome::Failed { .. } | TurnOutcome::Incomplete { .. } => {
            if !state.tool_replied {
                send_text(
                    sender,
                    state,
                    config.failure_text.clone(),
                    DeliveryStep::Failure,
                )
                .await;
            }
            finish(state);
        }
        TurnOutcome::ToolStarted { .. } | TurnOutcome::ToolFinished { .. } => {}
    }
}
pub(super) fn finish(state: &mut EventState) {
    state.terminal = true;
    state.terminal_at = Some(Instant::now());
    state.ack_due = None;
}
/// One event's timers: its due ack, or its next heartbeat. Returns `true`
/// when it sent, which ends the current tick's pass over every event.
pub(super) async fn timer_step(
    state: &mut EventState,
    sender: &Sender,
    config: &DeliveryConfig,
) -> bool {
    if state.terminal_at.is_some() {
        return false;
    }
    if state.ack_due.is_some_and(|at| at <= Instant::now()) {
        state.ack_due = None;
        if !state.first_send_done {
            state.ack_sent =
                send_text(sender, state, config.ack_text.clone(), DeliveryStep::Ack).await;
            state.last_progress_at = Instant::now();
            return true;
        }
    }
    if state.ack_sent
        && !state.tool_replied
        && !state.answer_attempted
        && state.heartbeats < 5
        && state.last_output_at.max(state.last_progress_at).elapsed() >= config.heartbeat
    {
        state.heartbeats += 1;
        send_text(
            sender,
            state,
            "Still working…".into(),
            DeliveryStep::Heartbeat,
        )
        .await;
        state.last_progress_at = Instant::now();
        return true;
    }
    false
}
async fn send_text(
    sender: &Sender,
    state: &mut EventState,
    text: String,
    step: DeliveryStep,
) -> bool {
    let message = OutboundMessage {
        whatsapp_template: None,
        reply_with_received_sticker: false,
        clear_native_attachments: false,
        native_media_update: None,
        native_attachments: Vec::new(),
        thread_reply: state.thread_reply,
        text: Some(text),
        blocks: vec![],
        reply_to: None,
    };
    let result = reply(sender, state, &message, step).await;
    matches!(result.delivery, DeliveryOutcome::Confirmed { .. })
}
/// Send `message` for `state`'s event through the legacy lane's `Ungated`
/// gate and apply the send's effects as soon as it returns. The serial
/// worker reads no event state while a send awaits, so reads and writes keep
/// the order they had when the send mutated the state. The outcome is
/// reported as `step` of the consumer bound to the event reference.
pub(super) async fn reply(
    sender: &Sender,
    state: &mut EventState,
    message: &OutboundMessage,
    step: DeliveryStep,
) -> IntentReceipt {
    let context = SendContext {
        trace: &state.trace,
        channel: &state.channel,
        first_send_done: state.first_send_done,
        gate: &Ungated,
    };
    let (result, effects) = sender.reply(&context, &state.event, message).await;
    state.first_send_done |= effects.first_send_done;
    sender.observe(&state.event, step, &result.delivery).await;
    result
}
