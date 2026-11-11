use super::*;
use crate::sandbox::conversation_recorder::live::{AgentLiveFragmentKind, AgentLiveHub};
use crate::AgentId;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

fn fragment(id: &str, text: &str) -> AgentLiveFragment {
    AgentLiveFragment {
        agent_id: AgentId(id.into()),
        runtime_generation: 1,
        turn_sequence: 1,
        after_seq: 3,
        message_id: "message".into(),
        kind: AgentLiveFragmentKind::Message,
        delta: text.into(),
    }
}

fn setup(bytes: usize) -> (Arc<LiveBudget>, AgentLiveHub) {
    let budget = Arc::new(LiveBudget::new(bytes));
    let hub = AgentLiveHub::with_budget(budget.clone());
    (budget, hub)
}

#[test]
fn retained_clone_holds_actual_string_capacity_until_last_drop() {
    let mut source = fragment("one", "private text");
    source.delta.reserve(1024);
    let bytes = retained_bytes(&source).unwrap() as usize;
    assert!(bytes > 1024);
    let budget = LiveBudget::new(bytes);
    let first = budget.retain(source).unwrap();
    let clone = first.clone();
    assert!(std::ptr::eq(first.deref(), clone.deref()));
    assert_eq!(budget.available(), 0);
    assert!(budget.retain(fragment("two", "next")).is_none());
    assert!(!format!("{clone:?}").contains("private text"));
    drop(first);
    assert_eq!(budget.available(), 0);
    drop(clone);
    assert_eq!(budget.available(), bytes);
}

#[tokio::test]
async fn lanes_share_capacity_and_return_ordered_gaps_without_blocking() {
    let source = fragment("one", "first");
    let bytes = retained_bytes(&source).unwrap() as usize;
    let (budget, hub) = setup(bytes);
    let mut first = hub.subscribe(&source.agent_id);
    let mut second = hub.subscribe(&AgentId("two".into()));
    hub.publish(Some(source));
    hub.publish(Some(fragment("two", "refused")));
    assert_eq!(second.recv().await, Err(RecvError::Lagged(1)));
    let held = first.recv().await.unwrap();
    assert_eq!(held.delta, "first");
    assert_eq!(budget.available(), 0);
    drop(held);
    assert_eq!(budget.available(), bytes);
    hub.publish(Some(fragment("two", "next")));
    assert_eq!(second.recv().await.unwrap().delta, "next");
    assert_eq!(budget.available(), bytes);
}

#[test]
fn subscriber_deferred_copy_retains_reservation_after_every_lane_closes() {
    let source = fragment("one", "first");
    let bytes = retained_bytes(&source).unwrap() as usize;
    let (budget, hub) = setup(bytes);
    let mut first = hub.subscribe(&source.agent_id);
    let mut second = hub.subscribe(&source.agent_id);
    hub.publish(Some(source));
    let held = first.try_recv().unwrap();
    let deferred = held.clone();
    let other = second.try_recv().unwrap();
    assert!(std::ptr::eq(held.deref(), other.deref()));
    drop((first, second, held, other));
    assert_eq!(budget.available(), 0);
    drop(deferred);
    assert_eq!(budget.available(), bytes);
}

#[test]
fn gap_is_after_the_accepted_frame_and_before_later_recovered_delivery() {
    let source = fragment("one", "first");
    let bytes = retained_bytes(&source).unwrap() as usize;
    let (budget, hub) = setup(bytes);
    let mut watching = hub.subscribe(&source.agent_id);
    hub.publish(Some(source));
    hub.publish(Some(fragment("one", "refused")));
    assert_eq!(watching.try_recv().unwrap().delta, "first");
    hub.publish(Some(fragment("one", "next")));
    assert_eq!(watching.try_recv(), Err(TryRecvError::Lagged(1)));
    assert_eq!(watching.try_recv().unwrap().delta, "next");
    assert_eq!(watching.try_recv(), Err(TryRecvError::Empty));
    assert_eq!(budget.available(), bytes);
}

#[test]
fn utf8_fragments_are_shared_and_bounded_without_unaccounted_coalescing() {
    let text = "€".repeat(7000);
    let (budget, hub) = setup(65536);
    let mut first = hub.subscribe(&AgentId("one".into()));
    let mut second = hub.subscribe(&AgentId("one".into()));
    hub.publish(Some(fragment("one", &text)));
    let mut result = String::new();
    let mut count = 0;
    while let Ok(mut delivery) = first.try_recv() {
        let duplicate = second.try_recv().unwrap();
        assert_eq!(delivery, duplicate);
        assert!(std::ptr::eq(delivery.deref(), duplicate.deref()));
        assert!(delivery.delta.len() <= 16384);
        assert!(!delivery.coalesce_legacy(&duplicate, usize::MAX));
        result.push_str(&delivery.delta);
        count += 1;
    }
    assert_eq!(result, text);
    assert_eq!(count, 2);
    assert_eq!(budget.available(), 65536);
}

#[test]
fn unsubscribe_eviction_and_no_observer_release_retained_bytes() {
    let (budget, hub) = setup(65536);
    hub.publish(Some(fragment("absent", "unobserved")));
    assert_eq!(budget.available(), 65536);
    let mut watching = hub.subscribe(&AgentId("one".into()));
    for _ in 0..300 {
        hub.publish(Some(fragment("one", "x")));
    }
    assert_eq!(watching.try_recv(), Err(TryRecvError::Lagged(44)));
    assert!(budget.available() < 65536);
    drop(watching);
    assert_eq!(budget.available(), 65536);
}

#[test]
fn independent_recorders_share_one_process_budget() {
    assert!(Arc::ptr_eq(&LiveBudget::process(), &LiveBudget::process()));
}

#[tokio::test]
async fn cancelling_a_waiting_subscription_does_not_retain_a_reservation() {
    let (budget, hub) = setup(4096);
    let mut watching = hub.subscribe(&AgentId("one".into()));
    let mut waiting = Box::pin(watching.recv());
    assert!(futures_util::poll!(&mut waiting).is_pending());
    drop(waiting);
    hub.publish(Some(fragment("one", "after cancellation")));
    assert_eq!(watching.recv().await.unwrap().delta, "after cancellation");
    drop(watching);
    assert_eq!(budget.available(), 4096);
}

#[test]
fn concurrent_lanes_cannot_overshoot_a_shared_reservation() {
    let bytes = retained_bytes(&fragment("one", "payload")).unwrap() as usize;
    let (budget, hub) = setup(bytes * 2);
    let hub = Arc::new(hub);
    let watchers: Vec<_> = (0..16)
        .map(|i| hub.subscribe(&AgentId(format!("{i:03}"))))
        .collect();
    std::thread::scope(|scope| {
        for i in 0..16 {
            let hub = hub.clone();
            scope.spawn(move || hub.publish(Some(fragment(&format!("{i:03}"), "payload"))));
        }
    });
    let mut deliveries = Vec::new();
    let mut gaps = 0;
    for mut watching in watchers {
        match watching.try_recv() {
            Ok(delivery) => deliveries.push(delivery),
            Err(TryRecvError::Lagged(1)) => gaps += 1,
            other => panic!("unexpected result: {other:?}"),
        }
    }
    assert_eq!(deliveries.len(), 2);
    assert_eq!(gaps, 14);
    assert_eq!(budget.available(), 0);
    drop(deliveries);
    assert_eq!(budget.available(), bytes * 2);
}

#[test]
fn disabled_hub_preserves_legacy_cloning_and_coalescing() {
    let hub = AgentLiveHub::default();
    let mut first = hub.subscribe(&AgentId("one".into()));
    let mut second = hub.subscribe(&AgentId("one".into()));
    hub.publish(Some(fragment("one", "text")));
    let mut left = first.try_recv().unwrap();
    let right = second.try_recv().unwrap();
    assert_eq!(left, right);
    assert!(!std::ptr::eq(left.deref(), right.deref()));
    assert!(left.coalesce_legacy(&right, 8));
    assert_eq!(left.delta, "texttext");
    assert!(!left.coalesce_legacy(&right, 8));
}
