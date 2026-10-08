//! The `[<label>] ` prefix that tells a chat which consumer it is reading.
use super::address::EffectiveAddress;
use crate::telemetry::names;
use cmaeg_core::{
    channel::{ChannelDefinition, ReplyLabel},
    ids::ConsumerId,
    seal::ReplyAddress,
};

/// A label is active when `routing_send` holds and either `reply_label` is
/// `always`, or it is `auto` and the channel has at least two non-pending
/// consumers. Never on unrouted channels. The label is the consumer's stored
/// label, else its name; `None` is `default`. A consumer the snapshot no
/// longer defines gets none.
pub(super) fn active_label(
    channel: &ChannelDefinition,
    consumer: Option<&ConsumerId>,
    routing_send: bool,
) -> Option<String> {
    if !routing_send {
        return None;
    }
    let routing = channel.routing.as_ref()?;
    let active = match routing.reply_label {
        ReplyLabel::Never => false,
        ReplyLabel::Always => true,
        ReplyLabel::Auto => {
            channel
                .consumers()
                .filter(|consumer| !consumer.pending)
                .count()
                >= 2
        }
    };
    if !active {
        return None;
    }
    channel.consumer(consumer).map(|view| view.label.to_owned())
}

/// The chat transport an address sends through, as a metric label.
pub(super) fn transport(address: &ReplyAddress) -> Option<&'static str> {
    match address {
        ReplyAddress::Telegram { .. } => Some("telegram"),
        ReplyAddress::NyxidRelay { .. } => Some("relay"),
        ReplyAddress::Sync { .. } | ReplyAddress::ResultWebhook { .. } | ReplyAddress::None => None,
    }
}

/// The chat transport a resolved address sends through, as a metric label:
/// its origin's, or the default chat's.
pub(super) fn effective_transport(address: &EffectiveAddress) -> Option<&'static str> {
    match address {
        EffectiveAddress::Origin(origin) => transport(origin),
        EffectiveAddress::DefaultTelegramChat { .. } => Some("telegram"),
        EffectiveAddress::DefaultRelayConversation { .. } => Some("relay"),
        EffectiveAddress::DefaultSinks | EffectiveAddress::Rejected { .. } => None,
    }
}

/// Count a chat send that carries `label`. Custom sinks are never labeled,
/// so `transport` is `None` for them and nothing is counted.
pub(super) fn count(label: Option<&str>, transport: Option<&'static str>) {
    if let (Some(_), Some(transport)) = (label, transport) {
        metrics::counter!(names::DELIVERY_LABELS, names::TRANSPORT => transport).increment(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const REVIEWER: &str = "csm_00000000000a";

    /// A routed channel whose reviewer is `pending` and labeled "rev", under `mode`.
    fn routed(mode: &str, pending: bool) -> ChannelDefinition {
        let mut value: Value = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        let reviewer = json!({"id": REVIEWER, "name": "reviewer", "label": "rev",
            "profile": value["profile"], "profile_digest": value["profile_digest"],
            "provider": value["provider"], "agent_key_hash": value["secrets"]["agent_key_hash"],
            "pending": pending, "created_at": "2026-10-01T00:00:00Z"});
        value["routing"] = json!({"version": 1, "default_consumer": {"name": "lead"},
            "consumers": [reviewer], "default_targets": ["default"], "reply_label": mode});
        serde_json::from_value(value).unwrap()
    }

    fn reviewer() -> ConsumerId {
        ConsumerId::parse(REVIEWER).unwrap()
    }

    #[test]
    fn auto_labels_only_with_two_non_pending_consumers() {
        let both = routed("auto", false);
        assert_eq!(active_label(&both, None, true).as_deref(), Some("lead"));
        assert_eq!(
            active_label(&both, Some(&reviewer()), true).as_deref(),
            Some("rev")
        );
        let one = routed("auto", true);
        assert_eq!(active_label(&one, None, true), None);
        assert_eq!(active_label(&one, Some(&reviewer()), true), None);
    }

    #[test]
    fn always_and_never_ignore_the_consumer_count() {
        let always = routed("always", true);
        assert_eq!(active_label(&always, None, true).as_deref(), Some("lead"));
        assert_eq!(
            active_label(&always, Some(&ConsumerId::default_consumer()), true).as_deref(),
            Some("lead")
        );
        assert_eq!(active_label(&routed("never", false), None, true), None);
    }

    #[test]
    fn no_label_without_a_routing_send_or_a_routed_channel() {
        assert_eq!(active_label(&routed("always", false), None, false), None);
        let mut unrouted = routed("always", false);
        unrouted.routing = None;
        assert_eq!(active_label(&unrouted, None, true), None);
        let unknown = ConsumerId::parse("csm_00000000000f").unwrap();
        assert_eq!(
            active_label(&routed("always", false), Some(&unknown), true),
            None
        );
    }

    #[test]
    fn labeled_chat_sends_are_counted_by_transport_only() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let metrics = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            count(Some("lead"), Some("relay"));
            count(Some("lead"), Some("telegram"));
            count(None, Some("relay"));
            count(Some("lead"), transport(&ReplyAddress::None));
        });
        let rendered = metrics.render();
        assert!(
            rendered.contains("cmaeg_delivery_labels_total{transport=\"relay\"} 1"),
            "{rendered}"
        );
        assert!(
            rendered.contains("cmaeg_delivery_labels_total{transport=\"telegram\"} 1"),
            "{rendered}"
        );
        assert_eq!(
            rendered.matches("cmaeg_delivery_labels_total{").count(),
            2,
            "{rendered}"
        );
        assert!(
            !rendered.contains("lead"),
            "a label is never a metric label: {rendered}"
        );
    }
}
