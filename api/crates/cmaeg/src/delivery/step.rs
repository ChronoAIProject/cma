//! Per-event delivery steps over an event's group and one of its consumers.
use super::{
    address::{
        count_unavailable, effective, requested, routing_send, AddressClass, EffectiveAddress,
        SendKind, DEFAULT_REPLY_UNAVAILABLE,
    },
    default_destination,
    gate::Ungated,
    group::EventGroup,
    intents::{DeliveryOutcome, IntentReceipt},
    label::{self, active_label},
    part_keys::PartKeys,
    send::{SendContext, SendEffects, Sender},
};
use crate::{turns::outcomes::TurnOutcome, DeliveryConfig};
use cmaeg_core::message::OutboundMessage;
use std::time::Duration;
use tokio::time::Instant;

/// The `kind` of an automatic output's idempotency keys: an answer, or the
/// failure text.
const ANSWER: &str = "answer";
const FAILURE: &str = "failure";

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
            // Progress goes only to a chat origin: a custom event answered
            // in a default chat never had an ack, and keeps none.
            if !group.handoff
                && AddressClass::of(&group.consumers[at].event.reply) == AddressClass::Chat
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
                send_output(sender, group, at, text, config, ANSWER).await;
            }
        }
        TurnOutcome::Completed { final_text } => {
            let delivery = &mut group.consumers[at];
            if delivery.policy.auto_final && !delivery.answer_attempted && !delivery.tool_replied {
                if let Some(text) = final_text {
                    delivery.answer_attempted = true;
                    send_output(sender, group, at, text, config, ANSWER).await;
                }
            }
            group.finish(at);
        }
        TurnOutcome::Failed { .. } | TurnOutcome::Incomplete { .. } => {
            if !group.consumers[at].tool_replied {
                let text = config.failure_text.clone();
                send_output(sender, group, at, text, config, FAILURE).await;
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
            let text = heartbeat(group, config.multi_consumer_enabled);
            send_progress(sender, group, text).await;
            group.last_progress_at = Instant::now();
            return true;
        }
    }
    false
}

/// The heartbeat text. With labels active it names the consumers still owed
/// an answer, ` (<labels, comma-separated>)`.
fn heartbeat(group: &EventGroup, multi_consumer_enabled: bool) -> String {
    let pending: Vec<String> = (0..group.consumers.len())
        .filter(|at| group.consumers[*at].pending())
        .filter_map(|at| output_label(group, at, multi_consumer_enabled))
        .collect();
    if pending.is_empty() {
        "Still working…".into()
    } else {
        format!("Still working… ({})", pending.join(", "))
    }
}

/// Whether consumer `at`'s automatic outputs are routed sends.
fn output_routing(group: &EventGroup, at: usize, multi_consumer_enabled: bool) -> bool {
    let delivery = &group.consumers[at];
    routing_send(
        &group.channel.definition,
        &delivery.event,
        multi_consumer_enabled,
        SendKind::Output {
            routed_turn: delivery.routed_turn,
        },
    )
}

/// Consumer `at`'s label on its automatic outputs, when one is active.
fn output_label(group: &EventGroup, at: usize, multi_consumer_enabled: bool) -> Option<String> {
    let routed = output_routing(group, at, multi_consumer_enabled);
    active_label(
        &group.channel.definition,
        Some(&group.consumers[at].consumer),
        routed,
    )
}

/// Where consumer `at`'s automatic outputs go, resolved now from the
/// group's snapshot and the origin's current reply capability.
pub(super) fn output_address(
    sender: &Sender,
    group: &EventGroup,
    at: usize,
    multi_consumer_enabled: bool,
) -> EffectiveAddress {
    let delivery = &group.consumers[at];
    effective(
        &group.channel.definition,
        &delivery.event,
        requested(None, delivery.policy.destination, &delivery.event),
        sender.custom.sync_registered(&delivery.event.reply),
        multi_consumer_enabled,
        SendKind::Output {
            routed_turn: delivery.routed_turn,
        },
    )
}

/// Record a rejected automatic output: nothing is sent, and the consumer is
/// terminal, so the event's other consumers and its expiry carry on.
fn reject_output(group: &mut EventGroup, at: usize, reason: &'static str) {
    if reason == DEFAULT_REPLY_UNAVAILABLE {
        count_unavailable(&group.channel.definition);
    }
    tracing::warn!(
        channel = %crate::telemetry::channel_hash(group.channel.id.as_str()),
        consumer = %group.consumers[at].consumer,
        reason,
        "automatic output rejected"
    );
    group.finish(at);
}

/// A custom-class consumer's outcome. Its Sync frame was already emitted;
/// the terminal outcome goes to the result webhooks, tagged as the default
/// destination when the consumer's answers go there.
pub(super) async fn custom_outcome(
    group: &mut EventGroup,
    at: usize,
    outcome: TurnOutcome,
    sender: &Sender,
    address: &EffectiveAddress,
) {
    if matches!(outcome, TurnOutcome::Accepted { .. }) {
        group.terminal_at = None;
    }
    match address {
        EffectiveAddress::Rejected { reason } => {
            if outcome.terminal() {
                reject_output(group, at, reason);
            }
            return;
        }
        EffectiveAddress::DefaultSinks => {
            let event = &group.consumers[at].event;
            let delivered = sender
                .custom
                .default_outcome(&group.channel, event, &outcome)
                .await;
            if let Some(delivered) = delivered {
                default_destination::record(&group.channel, "webhook", &delivered);
            }
        }
        _ => {
            sender
                .custom
                .outcome(&group.channel, &group.consumers[at].event, &outcome)
                .await
        }
    }
    if outcome.terminal() {
        group.finish(at);
    }
}

/// The event's progress (its ack or a heartbeat), sent once per event,
/// unlabeled, to the origin every consumer shares.
async fn send_progress(sender: &Sender, group: &mut EventGroup, text: String) -> bool {
    let origin = EffectiveAddress::Origin(group.consumers[0].event.reply.clone());
    send_text(sender, group, 0, text, None, &origin, None).await
}

/// Consumer `at`'s answer or failure text, opened with its label, at its
/// effective address. An output for an unavailable destination is rejected,
/// never redirected to the origin.
async fn send_output(
    sender: &Sender,
    group: &mut EventGroup,
    at: usize,
    text: String,
    config: &DeliveryConfig,
    kind: &'static str,
) -> bool {
    let flag = config.multi_consumer_enabled;
    let address = output_address(sender, group, at, flag);
    if let EffectiveAddress::Rejected { reason } = address {
        reject_output(group, at, reason);
        return false;
    }
    let label = output_label(group, at, flag);
    let keys = output_keys(group, at, kind);
    send_text(
        sender,
        group,
        at,
        text,
        label.as_deref(),
        &address,
        Some(&keys),
    )
    .await
}

/// The keys of consumer `at`'s next automatic output, numbered by its
/// output sequence so two committed messages never share a key.
fn output_keys(group: &mut EventGroup, at: usize, kind: &'static str) -> PartKeys {
    let delivery = &mut group.consumers[at];
    let seq = delivery.outputs;
    delivery.outputs += 1;
    PartKeys::Output {
        record_id: delivery.event.record_id.as_str().into(),
        event_id: delivery.event.event_id.as_str().into(),
        consumer: delivery.consumer.as_str().into(),
        kind,
        seq,
    }
}

async fn send_text(
    sender: &Sender,
    group: &mut EventGroup,
    at: usize,
    text: String,
    label: Option<&str>,
    address: &EffectiveAddress,
    keys: Option<&PartKeys>,
) -> bool {
    let message = OutboundMessage {
        text: Some(text),
        blocks: vec![],
        reply_to: None,
    };
    let result = reply(sender, group, at, &message, label, address, keys).await;
    matches!(result.delivery, DeliveryOutcome::Confirmed { .. })
}

/// Send `message` for consumer `at` of `group`'s event to `address`, opened
/// with `label`, through the `Ungated` gate, and apply the send's effects as
/// soon as it returns. `keys` number the parts of a default relay send. The
/// serial worker reads no group state while a send awaits, so reads and
/// writes keep the order they had when the send mutated the state.
pub(super) async fn reply(
    sender: &Sender,
    group: &mut EventGroup,
    at: usize,
    message: &OutboundMessage,
    label: Option<&str>,
    address: &EffectiveAddress,
    keys: Option<&PartKeys>,
) -> IntentReceipt {
    label::count(label, label::effective_transport(address));
    let context = SendContext {
        trace: &group.trace,
        channel: &group.channel,
        first_send_done: group.first_send_done,
        gate: &Ungated,
        label,
    };
    let event = &group.consumers[at].event;
    // A default destination answers no inbound message, so its sends never
    // mark the event's first send or spend the origin's reply token.
    let (result, effects) = match address {
        EffectiveAddress::Origin(_) => sender.reply(&context, event, message).await,
        _ => (
            sender
                .default_reply(&context, event, address, message, keys)
                .await,
            SendEffects::default(),
        ),
    };
    group.first_send_done |= effects.first_send_done;
    result
}
