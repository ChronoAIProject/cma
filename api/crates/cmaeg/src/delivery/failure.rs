//! A fan-out consumer that failed before its turn could report anything.
//!
//! Such a child has no sealed event_ref the worker could open (G47), so its
//! failure arrives as its own command and joins the event's group like any
//! outcome of that consumer. A consumer is terminal at most once per event:
//! a child the runner refused already reported its own terminal outcome,
//! and a second notice or webhook envelope for it would end a stream early.
use super::{
    address::AddressClass,
    event::key,
    group::{consumer_of, Groups},
    send::Sender,
    step::{custom_outcome, outcome_event, output_address},
};
use crate::{
    channels::resolver::ResolvedChannel, telemetry::channel_hash, turns::outcomes::TurnOutcome,
    DeliveryConfig,
};
use cmaeg_core::{
    ids::ConsumerId,
    seal::{EventRefPayload, ReplyAddress},
};
use std::sync::Arc;

/// The fields of a `DeliveryCommand::ConsumerFailed`.
pub(super) struct ConsumerFailure {
    pub channel: Arc<ResolvedChannel>,
    pub payload: EventRefPayload,
    pub consumer: ConsumerId,
    pub code: &'static str,
    pub provider_code: Option<String>,
}

/// A provider's own failure code, kept only when it is a bounded token. It
/// is logged and never shown to chat users.
pub(super) fn provider_code(code: Option<String>) -> Option<String> {
    code.filter(|code| {
        (1..=64).contains(&code.len())
            && code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    })
}

pub(super) async fn consumer_failed(
    groups: &mut Groups,
    sender: &Sender,
    config: &DeliveryConfig,
    failure: ConsumerFailure,
) {
    let ConsumerFailure {
        channel,
        payload,
        consumer,
        code,
        provider_code: reported,
    } = failure;
    let channel_hash = channel_hash(channel.id.as_str());
    if consumer_of(&payload) != consumer || payload.channel_id != channel.id {
        tracing::error!(
            channel = %channel_hash,
            %consumer,
            "consumer failure does not match its event; dropped"
        );
        return;
    }
    let provider_code = provider_code(reported);
    tracing::warn!(
        channel = %channel_hash,
        %consumer,
        code,
        provider_code = provider_code.as_deref(),
        "fan-out consumer failed"
    );
    let outcome = TurnOutcome::Failed {
        code: code.into(),
        message: "the consumer's turn did not start".into(),
    };
    // Before capacity is checked, so a Sync waiter always receives this
    // consumer's terminal frame, as a `StreamOutcome`'s frame is emitted.
    if matches!(payload.reply, ReplyAddress::Sync { .. }) {
        sender.custom.stream(&payload, &outcome);
    }
    let id = key(payload.channel_id.as_str(), payload.event_id.as_str());
    if !groups.reserve(&id, &consumer) {
        tracing::warn!("delivery event capacity reached");
        return;
    }
    let (group, at) = groups.join(id, &channel, &payload);
    if group.consumers[at].terminal {
        tracing::info!(
            channel = %channel_hash,
            %consumer,
            "consumer already terminal; its failure is not repeated"
        );
        return;
    }
    // A fan-out child is a routed turn by construction.
    group.consumers[at].routed_turn = true;
    let address = output_address(group, at, config.multi_consumer_enabled);
    let class = address
        .class()
        .unwrap_or_else(|| AddressClass::of(&group.consumers[at].event.reply));
    match class {
        AddressClass::Custom => custom_outcome(group, at, outcome, sender, &address).await,
        AddressClass::Chat => outcome_event(group, at, outcome, sender, config).await,
    }
}

#[cfg(test)]
#[path = "failure_tests.rs"]
mod tests;
