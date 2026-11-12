//! Report logical delivery-step results, separately from HTTP and queue success.
use crate::{
    delivery::{destination::DefaultDestination, intents::DeliveryOutcome},
    turns::observer::{DeliveryStep, Destination},
};
use cmaeg_core::seal::ReplyAddress;
pub(crate) fn outcome(
    step: DeliveryStep,
    result: &DeliveryOutcome,
    destination: Destination,
    reply: &ReplyAddress,
    default: Option<&DefaultDestination>,
) {
    let platform = match default {
        Some(DefaultDestination::Telegram { .. }) => "telegram",
        Some(DefaultDestination::Relay { platform, .. }) => platform.as_str(),
        Some(DefaultDestination::Webhooks { .. }) => "webhook_group",
        None => match reply {
            ReplyAddress::NativeLark(_) => "lark",
            ReplyAddress::Whatsapp { .. } | ReplyAddress::WhatsappGroup(_) => "whatsapp",
            ReplyAddress::Telegram { .. } => "telegram",
            ReplyAddress::Discord { .. } => "discord",
            ReplyAddress::NyxidRelay { platform, .. } => platform.as_str(),
            ReplyAddress::ResultWebhook { .. } => "webhook",
            ReplyAddress::Sync { .. } => "local_stream",
            ReplyAddress::None => "none",
        },
    };
    record(step, result.kind(), destination, platform);
}
pub(crate) fn webhook(step: DeliveryStep, result: &DeliveryOutcome) {
    record(step, result.kind(), Destination::Origin, "webhook");
}
pub(crate) fn local(buffered: bool) {
    record(
        DeliveryStep::Answer,
        if buffered { "buffered" } else { "unavailable" },
        Destination::Origin,
        "local_stream",
    );
}
fn record(
    step: DeliveryStep,
    outcome: &'static str,
    destination: Destination,
    platform: &'static str,
) {
    let step = match step {
        DeliveryStep::Ack => "ack",
        DeliveryStep::Heartbeat => "heartbeat",
        DeliveryStep::Answer => "answer",
        DeliveryStep::ToolReply => "tool_reply",
        DeliveryStep::Failure => "failure",
        DeliveryStep::DefaultReply => "default_reply",
    };
    let destination = match destination {
        Destination::Origin => "origin",
        Destination::Default => "default",
    };
    metrics::counter!("cmaeg_delivery_outcomes_total", "step" => step, "outcome" => outcome, "destination" => destination, "platform" => platform).increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_unknown_transport_from_local_buffering_without_recipient_labels() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            outcome(
                DeliveryStep::Answer,
                &DeliveryOutcome::Unknown {
                    reason: "private failure detail".into(),
                },
                Destination::Origin,
                &ReplyAddress::ResultWebhook {
                    url: "https://private.example/recipient".into(),
                    hmac_key_id: "private-key-name".into(),
                },
                None,
            );
            local(true);
            local(false);
        });
        let text = handle.render();
        for fixed in [
            "outcome=\"unknown\"",
            "outcome=\"buffered\"",
            "outcome=\"unavailable\"",
            "platform=\"webhook\"",
            "platform=\"local_stream\"",
        ] {
            assert!(text.contains(fixed), "{text}");
        }
        assert!(!text.contains("private"));
        assert!(!text.contains("confirmed"));
    }
}
