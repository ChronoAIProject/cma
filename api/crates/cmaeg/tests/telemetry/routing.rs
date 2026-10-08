//! Exercise telemetry through the real resolver with fixed-cardinality labels.
use super::{captured, names, policy::REVIEWER, routing_fixture as f, telemetry};
use cmaeg::{channels::resolver::ResolvedChannel, telemetry::routing::resolve};
use cmaeg_core::{channel::ChannelDefinition, seal::ChannelPointer};
use serde_json::json;

fn channel(definition: ChannelDefinition) -> ResolvedChannel {
    ResolvedChannel {
        id: "cmaeg1.ch.private-channel".parse().unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/private-record".into(),
        },
        definition,
        agent_key_hash: [0; 32],
        consumer_keys: Vec::new(),
        loaded_at: std::time::Instant::now(),
        version: 1,
    }
}

#[test]
fn decisions_use_only_fixed_labels_and_hash_sensitive_identifiers() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut expected = Vec::new();
    let logs = captured(Some("trace"), || {
        metrics::with_local_recorder(&recorder, || {
            let normal = channel(f::channel(|_| {}));
            let ruled = channel(f::channel(|raw| {
                raw["routing"]["rules"] =
                    json!([f::rule("safe-rule", json!({}), json!(["default"]))])
            }));
            let legacy = channel(f::unrouted());
            let unknown = "unknown-private-token";
            let reviewer = cmaeg_core::ids::ConsumerId::parse(REVIEWER).unwrap();
            let tokens = vec!["reviewer".into()];
            let missing = vec![unknown.into()];
            for (channel, enabled, text, resolution, outcome) in [
                (&legacy, true, "private words", "legacy", "legacy"),
                (
                    &normal,
                    false,
                    "@main private words",
                    "kill_switch",
                    "legacy",
                ),
                (
                    &normal,
                    false,
                    "@reviewer private words",
                    "kill_switch",
                    "refused",
                ),
                (&normal, true, "private words", "default", "routed"),
                (&ruled, true, "@unknown private words", "rule", "routed"),
                (
                    &normal,
                    true,
                    "@reviewer private words",
                    "mention",
                    "routed",
                ),
                (
                    &normal,
                    true,
                    "/to reviewer private words",
                    "command",
                    "routed",
                ),
                (
                    &normal,
                    true,
                    "/to unknown-private-token private words",
                    "command",
                    "refused",
                ),
            ] {
                let mut activity = f::message(f::telegram(), text);
                activity.event_id = "private-person@example.test".parse().unwrap();
                let result = resolve(channel, &f::input(&channel.definition, &activity), enabled);
                assert_eq!(result.is_err(), outcome == "refused");
                expected.push((resolution, outcome));
            }
            for (activity, resolution, refused) in [
                (
                    f::event(
                        "private-event-type",
                        json!({"cmaegtargets":"reviewer","data":{"private":"payload"}}),
                    ),
                    "event_field",
                    false,
                ),
                (
                    f::event("private-event-type", json!({"cmaegtargets":unknown})),
                    "event_field",
                    true,
                ),
                (f::click(f::telegram()), "interaction", false),
                (f::click(f::telegram()), "interaction", true),
                (f::handoff("private handoff"), "agent_event", false),
                (f::handoff("private handoff"), "agent_event", true),
            ] {
                let mut input = f::input(&normal.definition, &activity);
                if resolution == "interaction" && !refused {
                    input.card_consumer = Some(Some(&reviewer));
                }
                if resolution == "agent_event" {
                    input.published =
                        Some((if refused { &missing } else { &tokens }, f::causation()));
                }
                assert_eq!(resolve(&normal, &input, true).is_err(), refused);
                expected.push((resolution, if refused { "refused" } else { "routed" }));
            }
        })
    });
    let metrics = handle.render();
    let samples: Vec<_> = metrics
        .lines()
        .filter(|line| line.starts_with(names::ROUTING_DECISIONS))
        .collect();
    assert_eq!(samples.len(), expected.len(), "{metrics}");
    for (resolution, outcome) in expected {
        assert!(
            samples.iter().any(
                |line| line.contains(&format!("resolution=\"{resolution}\""))
                    && line.contains(&format!("outcome=\"{outcome}\""))
                    && line.ends_with(" 1")
            ),
            "{metrics}"
        );
    }
    assert!(super::label_keys(&metrics, names::ROUTING_DECISIONS)
        .iter()
        .all(|key| ["resolution", "outcome"].contains(key)));
    for hidden in [
        "private-person@example.test",
        "private words",
        "private-event-type",
        "payload",
        "unknown-private-token",
        "private handoff",
        "cmaeg1.ch.private-channel",
        "fixture-channel-key",
        "nyx_fixture_",
    ] {
        assert!(!logs.contains(hidden), "leaked {hidden}: {logs}");
        assert!(!metrics.contains(hidden), "leaked {hidden}: {metrics}");
    }
    assert!(logs.contains(&telemetry::channel_hash("private-person@example.test")));
    assert!(logs.contains("cmaeg.route"));
    assert!(logs.contains("safe-rule"));
    assert!(logs.contains(REVIEWER));
    assert!(
        logs.lines().any(|line| line.contains("WARN")
            && line.contains("resolution=\"mention\"")
            && line.contains("transport=\"telegram\"")
            && line.contains("kill-switch routing refusal")),
        "{logs}"
    );
}
