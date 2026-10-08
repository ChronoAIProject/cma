//! Plain and multiplexed waiters share the existing process-wide limits.
use super::*;
use axum::response::{sse::Sse, IntoResponse};

#[tokio::test]
async fn queue_width_and_the_aggregate_terminal_are_both_bounded() {
    let registry = Arc::new(SyncRegistry::default());
    for count in [1, 2, 8] {
        let consumers: Vec<_> = (0..count)
            .map(|at| summary(&format!("csm_{at:012x}"), &format!("worker{at}")))
            .collect();
        let (id, waiter) = fanout(&registry, &consumers);
        for _ in 0..8 * count {
            assert!(registry.emit(&id, "cmaeg.text.delta", &data(&consumers[0]), false));
        }
        assert!(!registry.emit(&id, "cmaeg.text.delta", &data(&consumers[0]), false));
        assert!(waiter.failed.load(Ordering::Acquire));
        assert!(!registry.registered(&id));
        drop(waiter);
        assert_eq!(registry.bytes.available_permits(), 32 * 1024 * 1024);
    }
    let (id, waiter) = registry.register_fanout(1, default()).unwrap();
    for _ in 0..7 {
        assert!(registry.emit(&id, "cmaeg.message", &json!({}), false));
    }
    // The child terminal fits in slot eight; its aggregate needs slot nine.
    assert!(!registry.emit(&id, "cmaeg.completed", &json!({}), true));
    let body = axum::body::to_bytes(Sse::new(waiter.stream()).into_response().into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(String::from_utf8(body.to_vec()).unwrap(),
        "event: cmaeg.failed\ndata: {\"error\":{\"code\":\"internal\",\"detail\":\"sync_backpressure\"}}\n\n");
    assert_eq!(registry.bytes.available_permits(), 32 * 1024 * 1024);
}

#[test]
fn shared_byte_budget_and_frame_limit_include_translated_and_final_frames() {
    let registry = Arc::new(SyncRegistry::default());
    let mut held = vec![];
    let mut full = json!({"text":""});
    full["text"] = "x".repeat(1024 * 1024 - full.to_string().len()).into();
    assert_eq!(full.to_string().len(), 1024 * 1024);
    for _ in 0..4 {
        let (id, waiter) = registry.register().unwrap();
        for _ in 0..8 {
            assert!(registry.emit(&id, "cmaeg.text.delta", &full, false));
        }
        held.push(waiter);
    }
    assert_eq!(registry.bytes.available_permits(), 0);
    let (id, waiter) = registry.register_fanout(1, default()).unwrap();
    assert!(!registry.emit(&id, "cmaeg.message", &json!({}), false));
    assert!(waiter.failed.load(Ordering::Acquire));
    drop(held);
    assert_eq!(registry.bytes.available_permits(), 32 * 1024 * 1024);
    // Injection itself is part of the one-MiB frame bound.
    let (id, waiter) = registry.register_fanout(1, default()).unwrap();
    assert!(!registry.emit(&id, "cmaeg.message", &full, false));
    assert!(waiter.failed.load(Ordering::Acquire));
    assert_eq!(registry.bytes.available_permits(), 32 * 1024 * 1024);
    let (id, waiter) = registry.register_fanout(1, default()).unwrap();
    assert!(registry.emit(&id, "cmaeg.completed", &json!({"event_id":"last"}), true));
    let charged: usize = waiter.receiver.len();
    assert_eq!(charged, 2);
    let bytes: usize = registry.bytes.available_permits();
    assert!(bytes < 32 * 1024 * 1024);
    drop(waiter);
    assert_eq!(registry.bytes.available_permits(), 32 * 1024 * 1024);
}

#[test]
fn the_final_aggregate_must_acquire_its_own_shared_byte_permit() {
    let registry = Arc::new(SyncRegistry::default());
    let mut held = vec![];
    let mut full = json!({"text":""});
    full["text"] = "x".repeat(1024 * 1024 - full.to_string().len()).into();
    for count in [8, 8, 8, 7] {
        let (id, waiter) = registry.register().unwrap();
        for _ in 0..count {
            assert!(registry.emit(&id, "cmaeg.message", &full, false));
        }
        held.push(waiter);
    }
    assert_eq!(registry.bytes.available_permits(), 1024 * 1024);
    let (id, waiter) = registry.register_fanout(1, default()).unwrap();
    let mut terminal = data(&default());
    terminal["text"] = "".into();
    terminal["text"] = "x".repeat(1024 * 1024 - terminal.to_string().len()).into();
    assert!(!registry.emit(&id, "cmaeg.completed", &terminal, true));
    assert_eq!(waiter.receiver.len(), 1, "only the child terminal fit");
    assert_eq!(registry.bytes.available_permits(), 0);
    assert!(waiter.failed.load(Ordering::Acquire));
    assert!(!registry.registered(&id));
    drop(waiter);
    drop(held);
    assert_eq!(registry.bytes.available_permits(), 32 * 1024 * 1024);
}

#[test]
fn plain_and_fanout_registration_share_the_same_waiter_limit() {
    let registry = Arc::new(SyncRegistry::default());
    let mut held = vec![];
    for at in 0..4096 {
        held.push(if at % 2 == 0 {
            registry.register().unwrap().1
        } else {
            registry.register_fanout(8, default()).unwrap().1
        });
    }
    assert!(registry.register().is_none());
    assert!(registry.register_fanout(1, default()).is_none());
    drop(held.pop());
    let (id, waiter) = registry.register_fanout(1, default()).unwrap();
    assert!(registry.registered(&id));
    drop(waiter);
    assert!(!registry.registered(&id));
    drop(held);
    assert!(registry.is_empty());
}
