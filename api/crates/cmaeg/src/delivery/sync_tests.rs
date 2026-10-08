//! Multiplexed identity, terminal accounting and unchanged plain streaming.
use super::*;
use cmaeg_core::ids::{ConsumerId, ConsumerName};
use serde_json::json;

#[path = "sync_limits_tests.rs"]
mod limits;

fn summary(id: &str, name: &str) -> ConsumerSummary {
    ConsumerSummary {
        id: ConsumerId::parse(id).unwrap(),
        name: ConsumerName::parse(name).unwrap(),
        label: name.into(),
    }
}
fn default() -> ConsumerSummary {
    summary("default", "main")
}
fn consumers() -> Vec<ConsumerSummary> {
    vec![
        default(),
        summary("csm_00000000000a", "reviewer"),
        summary("csm_00000000000b", "editor"),
    ]
}
fn targets(consumers: &[ConsumerSummary]) -> Vec<ConsumerTarget> {
    consumers
        .iter()
        .map(|consumer| ConsumerTarget {
            id: consumer.id.clone(),
            name: consumer.name.clone(),
        })
        .collect()
}
fn data(consumer: &ConsumerSummary) -> Value {
    json!({"event_id":"event-1","conversation_id":consumer.name,"consumer":consumer,"text":"answer"})
}
fn fanout(registry: &Arc<SyncRegistry>, consumers: &[ConsumerSummary]) -> (String, Waiter) {
    let (id, waiter) = registry
        .register_fanout(consumers.len() as u8, default())
        .unwrap();
    assert!(registry.set_fanout_targets(&id, &targets(consumers)));
    (id, waiter)
}
async fn frames(mut waiter: Waiter) -> Vec<(String, Value, bool)> {
    let mut frames = vec![];
    while let Some(frame) = waiter.receiver.recv().await {
        frames.push((
            frame.name.into(),
            serde_json::from_str(&frame.data).unwrap(),
            frame.terminal,
        ));
    }
    assert!(!waiter.failed.load(Ordering::Acquire));
    frames
}

#[tokio::test]
async fn interleaved_consumers_finish_once_in_pinned_target_order() {
    let registry = Arc::new(SyncRegistry::default());
    let consumers = consumers();
    let (id, waiter) = fanout(&registry, &consumers);
    for at in [1, 0, 2, 1, 2, 0] {
        assert!(registry.emit(&id, "cmaeg.text.delta", &data(&consumers[at]), false));
    }
    for at in [2, 0, 1] {
        assert!(registry.emit(&id, "cmaeg.message", &data(&consumers[at]), false));
        assert!(registry.emit(&id, "cmaeg.completed", &data(&consumers[at]), true));
    }
    assert!(registry.is_empty());
    let frames = frames(waiter).await;
    assert_eq!(frames.len(), 13);
    assert!(frames[..6]
        .iter()
        .all(|frame| frame.0 == "cmaeg.consumer.delta"));
    for (pair, at) in frames[6..12].chunks_exact(2).zip([2, 0, 1]) {
        assert_eq!(pair[0].0, "cmaeg.consumer.message");
        assert_eq!(pair[1].0, "cmaeg.consumer.completed");
        assert_eq!(pair[1].1["consumer"], json!(consumers[at]));
    }
    assert!(frames[..12].iter().all(|frame| !frame.2));
    assert_eq!(frames[12].0, "cmaeg.fanout.completed");
    assert!(frames[12].2);
    assert_eq!(
        frames[12].1,
        json!({"event_id":"event-1","consumers":[
            {"id":"default","name":"main","outcome":"completed"},
            {"id":"csm_00000000000a","name":"reviewer","outcome":"completed"},
            {"id":"csm_00000000000b","name":"editor","outcome":"completed"}
        ]})
    );
}

#[tokio::test]
async fn repeated_terminals_are_handled_but_late_messages_are_not_delivered() {
    let registry = Arc::new(SyncRegistry::default());
    let consumers = consumers();
    let (id, waiter) = fanout(&registry, &consumers);
    let mut refused = data(&consumers[1]);
    refused["refused"] = true.into();
    assert!(registry.emit(&id, "cmaeg.failed", &refused, true));
    let available = registry.bytes.available_permits();
    for _ in 0..100 {
        assert!(registry.emit(&id, "cmaeg.completed", &data(&consumers[1]), true));
        assert!(!registry.emit(&id, "cmaeg.message", &data(&consumers[1]), false));
        assert!(!registry.emit(&id, "cmaeg.text.delta", &data(&consumers[1]), false));
    }
    assert_eq!(registry.bytes.available_permits(), available);
    assert!(registry.registered(&id));
    assert!(registry.emit(&id, "cmaeg.failed", &data(&consumers[2]), true));
    assert!(registry.emit(&id, "cmaeg.completed", &data(&consumers[0]), true));
    let frames = frames(waiter).await;
    assert_eq!(frames.len(), 4);
    assert_eq!(frames[0].0, "cmaeg.consumer.failed");
    assert_eq!(frames[3].1["consumers"][1]["outcome"], "refused");
    assert_eq!(frames[3].1["consumers"][2]["outcome"], "failed");
}

#[tokio::test]
async fn single_default_translates_while_plain_frames_are_byte_identical() {
    let registry = Arc::new(SyncRegistry::default());
    for multiplex in [false, true] {
        let (id, waiter) = if multiplex {
            registry.register_fanout(1, default()).unwrap()
        } else {
            registry.register().unwrap()
        };
        let data = json!({"event_id":"event-1","text":"hello"});
        for (name, terminal) in [
            ("cmaeg.text.delta", false),
            ("cmaeg.message", false),
            ("cmaeg.completed", true),
        ] {
            assert!(registry.emit(&id, name, &data, terminal));
        }
        let frames = frames(waiter).await;
        assert_eq!(frames.len(), if multiplex { 4 } else { 3 });
        for (frame, suffix) in frames[..3].iter().zip(["delta", "message", "completed"]) {
            if multiplex {
                assert_eq!(frame.0, format!("cmaeg.consumer.{suffix}"));
                assert_eq!(frame.1["consumer"], json!(default()));
            } else {
                assert_eq!(frame.1.to_string(), data.to_string());
            }
        }
        if !multiplex {
            assert_eq!(frames[0].0, "cmaeg.text.delta");
            assert_eq!(frames[1].0, "cmaeg.message");
            assert_eq!(frames[2].0, "cmaeg.completed");
            assert!(frames[2].2);
        }
    }
}

#[tokio::test]
async fn routing_targets_can_replace_the_single_default_only_before_emission() {
    let registry = Arc::new(SyncRegistry::default());
    let consumers = consumers();
    let (id, waiter) = fanout(&registry, &consumers[1..2]);
    assert!(!registry.set_fanout_targets(&id, &targets(&consumers[..1])));
    assert!(registry.emit(&id, "cmaeg.completed", &data(&consumers[1]), true));
    assert_eq!(
        frames(waiter).await[1].1["consumers"][0]["name"],
        "reviewer"
    );
    let (id, _waiter) = registry.register_fanout(1, default()).unwrap();
    assert!(registry.emit(&id, "cmaeg.text.delta", &json!({}), false));
    assert!(!registry.set_fanout_targets(&id, &targets(&consumers[1..2])));
}

#[test]
fn invalid_registration_and_target_setup_do_not_mutate_existing_waiters() {
    let registry = Arc::new(SyncRegistry::default());
    assert!(registry.register_fanout(0, default()).is_none());
    assert!(registry.register_fanout(9, default()).is_none());
    assert!(registry
        .register_fanout(1, consumers()[1].clone())
        .is_none());
    assert!(registry.is_empty());
    let (id, _waiter) = registry.register_fanout(2, default()).unwrap();
    assert!(!registry.set_fanout_targets(&id, &targets(&consumers()[..1])));
    assert!(!registry.set_fanout_targets(&id, &targets(&[default(), default()])));
    assert!(registry.set_fanout_targets(&id, &targets(&consumers()[..2])));
    let (plain, _held) = registry.register().unwrap();
    assert!(!registry.set_fanout_targets(&plain, &targets(&consumers()[..1])));
    assert!(!registry.set_fanout_targets("absent", &[]));
}

#[test]
fn malformed_identity_unknown_consumer_and_unconfigured_targets_fail_explicitly() {
    let registry = Arc::new(SyncRegistry::default());
    for value in [
        json!(null),
        json!({"consumer":null}),
        json!({"consumer":{"id":"default","name":"main"}}),
        json!({"consumer":{"id":"invalid","name":"main","label":"Main"}}),
        json!({"consumer":{"id":"default","name":"BAD NAME","label":"Main"}}),
        data(&consumers()[1]),
    ] {
        let (id, waiter) = registry.register_fanout(1, default()).unwrap();
        assert!(!registry.emit(&id, "cmaeg.completed", &value, true));
        assert!(waiter.failed.load(Ordering::Acquire));
        assert!(!registry.registered(&id));
    }
    for (count, name, terminal) in [
        (2, "cmaeg.completed", true),
        (1, "cmaeg.unknown", false),
        (1, "cmaeg.message", true),
        (1, "cmaeg.failed", false),
    ] {
        let (id, waiter) = registry.register_fanout(count, default()).unwrap();
        assert!(!registry.emit(&id, name, &data(&default()), terminal));
        assert!(waiter.failed.load(Ordering::Acquire));
    }
}
