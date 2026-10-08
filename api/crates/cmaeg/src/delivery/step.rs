//! Per-event delivery steps over an event's group and one of its consumers.
use super::{
    gate::Ungated,
    group::EventGroup,
    intents::{DeliveryOutcome, IntentReceipt},
    send::{SendContext, Sender},
};
use crate::{turns::outcomes::TurnOutcome, DeliveryConfig};
use cmaeg_core::message::OutboundMessage;
use std::time::Duration;
use tokio::time::Instant;

/// Apply consumer `at`'s turn outcome: the event's ack is scheduled once, and
/// answers, auto-final and the failure text are that consumer's alone.
pub(super) async fn outcome_event(
    group: &mut EventGroup,
    at: usize,
    outcome: TurnOutcome,
    sender: &Sender,
    config: &DeliveryConfig,
) {
    match outcome {
        TurnOutcome::Accepted { .. } => {
            group.terminal_at = None;
            // Once per event: a second consumer's acceptance never schedules
            // another ack, and a hand-off has no human waiting for one.
            if !group.handoff
                && !group.ack_sent
                && group.ack_due.is_none()
                && !group.first_send_done
                && group.channel.definition.reply.ack_after_ms > 0
            {
                group.ack_due = Some(
                    Instant::now()
                        + Duration::from_millis(group.channel.definition.reply.ack_after_ms.into()),
                );
            }
        }
        TurnOutcome::TextDelta { .. } => {
            group.consumers[at].last_output_at = Instant::now();
        }
        TurnOutcome::MessageCommitted { text } => {
            let delivery = &mut group.consumers[at];
            delivery.last_output_at = Instant::now();
            if delivery.policy.auto_final && !delivery.tool_replied {
                delivery.answer_attempted = true;
                send_text(sender, group, at, text).await;
            }
        }
        TurnOutcome::Completed { final_text } => {
            let delivery = &mut group.consumers[at];
            if delivery.policy.auto_final && !delivery.answer_attempted && !delivery.tool_replied {
                if let Some(text) = final_text {
                    delivery.answer_attempted = true;
                    send_text(sender, group, at, text).await;
                }
            }
            group.finish(at);
        }
        TurnOutcome::Failed { .. } | TurnOutcome::Incomplete { .. } => {
            if !group.consumers[at].tool_replied {
                send_text(sender, group, at, config.failure_text.clone()).await;
            }
            group.finish(at);
        }
        TurnOutcome::ToolStarted { .. } | TurnOutcome::ToolFinished { .. } => {}
    }
}

/// One event's timers: its due ack, or its next heartbeat. Returns `true`
/// when it sent, which ends the current tick's pass over every event.
pub(super) async fn timer_step(
    group: &mut EventGroup,
    sender: &Sender,
    config: &DeliveryConfig,
) -> bool {
    if group.terminal_at.is_some() {
        return false;
    }
    if group.ack_due.is_some_and(|at| at <= Instant::now()) {
        group.ack_due = None;
        if !group.first_send_done {
            group.ack_sent = send_progress(sender, group, config.ack_text.clone()).await;
            group.last_progress_at = Instant::now();
            return true;
        }
    }
    // Heartbeats follow the consumers still owed an answer, so one quiet
    // consumer keeps them going after another has answered.
    let quiet_since = group
        .consumers
        .iter()
        .filter(|delivery| delivery.pending())
        .map(|delivery| delivery.last_output_at)
        .max();
    if let Some(last_output_at) = quiet_since {
        if group.ack_sent
            && group.heartbeats < 5
            && last_output_at.max(group.last_progress_at).elapsed() >= config.heartbeat
        {
            group.heartbeats += 1;
            send_progress(sender, group, "Still working…".into()).await;
            group.last_progress_at = Instant::now();
            return true;
        }
    }
    false
}

/// The event's progress (its ack or a heartbeat), sent once per event to the
/// origin every consumer shares.
async fn send_progress(sender: &Sender, group: &mut EventGroup, text: String) -> bool {
    send_text(sender, group, 0, text).await
}

async fn send_text(sender: &Sender, group: &mut EventGroup, at: usize, text: String) -> bool {
    let message = OutboundMessage {
        text: Some(text),
        blocks: vec![],
        reply_to: None,
    };
    let result = reply(sender, group, at, &message).await;
    matches!(result.delivery, DeliveryOutcome::Confirmed { .. })
}

/// Send `message` for consumer `at` of `group`'s event through the
/// `Ungated` gate and apply the send's effects as soon as it returns. The
/// serial worker reads no group state while a send awaits, so reads and
/// writes keep the order they had when the send mutated the state.
pub(super) async fn reply(
    sender: &Sender,
    group: &mut EventGroup,
    at: usize,
    message: &OutboundMessage,
) -> IntentReceipt {
    let context = SendContext {
        trace: &group.trace,
        channel: &group.channel,
        first_send_done: group.first_send_done,
        gate: &Ungated,
    };
    let (result, effects) = sender
        .reply(&context, &group.consumers[at].event, message)
        .await;
    group.first_send_done |= effects.first_send_done;
    result
}
