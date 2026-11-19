use super::*;
use crate::sandbox::conversation_recorder::live::{AgentLiveFragmentKind, AgentLiveHub};
use crate::sandbox::ingress_ownership::{AllocationPlan, ClassBounds};
use crate::AgentId;
use tokio::sync::broadcast::error::TryRecvError;

fn fragment(delta: &str) -> AgentLiveFragment {
    AgentLiveFragment {
        agent_id: AgentId("synthetic-agent".into()),
        runtime_generation: 3,
        turn_sequence: 7,
        after_seq: 11,
        message_id: "synthetic-message".into(),
        kind: AgentLiveFragmentKind::Message,
        delta: delta.into(),
    }
}

fn policy(data_bytes: usize) -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(data_bytes + 2048);
    let bounds = ClassBounds::new(
        data_bytes + 2048,
        AllocationPlan::new("control", &[1024]).unwrap(),
        AllocationPlan::new("transformation", &[1024]).unwrap(),
    )
    .unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    (root, policy)
}

fn setup(local_bytes: usize, policy: &IngressOwnershipPolicy) -> (Arc<LiveBudget>, AgentLiveHub) {
    let budget = Arc::new(LiveBudget {
        local: Arc::new(Semaphore::new(local_bytes)),
        aggregate: Some(policy.budget(IngressClass::Data)),
        pre_admit: true,
    });
    let hub = AgentLiveHub::with_budget(budget.clone());
    (budget, hub)
}

#[test]
fn data_pressure_preserves_control_and_gaps_are_ordered() {
    let source = fragment("first");
    let bytes = copied_bytes(&source, source.delta.len()).unwrap();
    let (root, policy) = policy(bytes);
    let (budget, hub) = setup(bytes * 2, &policy);
    let mut watching = hub.subscribe(&source.agent_id);
    hub.publish(Some(source));
    hub.publish(Some(fragment("refused")));
    assert_eq!(root.available_bytes(), 2048);
    assert_eq!(budget.available(), bytes);
    let control = policy
        .reserve(
            IngressClass::Control,
            AllocationPlan::new("control", &[1024]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![7u8; 128]);
    assert_eq!(control.len(), 128);
    assert_eq!(watching.try_recv().unwrap().delta, "first");
    assert_eq!(watching.try_recv(), Err(TryRecvError::Lagged(1)));
    hub.publish(Some(fragment("later")));
    assert_eq!(watching.try_recv().unwrap().delta, "later");
    drop(control);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert_eq!(budget.available(), bytes * 2);
}

#[test]
fn copies_use_length_but_shared_owners_keep_storage_after_every_lane_closes() {
    let mut source = fragment("retained");
    source.delta.reserve(32768);
    source.agent_id.0.reserve(8192);
    source.message_id.reserve(8192);
    let bytes = copied_bytes(&source, source.delta.len()).unwrap();
    let (root, policy) = policy(bytes);
    let (budget, hub) = setup(bytes, &policy);
    let mut first = hub.subscribe(&source.agent_id);
    let mut second = hub.subscribe(&source.agent_id);
    hub.publish(Some(source));
    let mut held = first.try_recv().unwrap();
    let other = second.try_recv().unwrap();
    assert!(std::ptr::eq(held.deref(), other.deref()));
    assert!(!held.coalesce_legacy(&other, usize::MAX));
    assert_eq!(held.delta.capacity(), held.delta.len());
    assert_eq!(held.agent_id.0.capacity(), held.agent_id.0.len());
    assert_eq!(held.message_id.capacity(), held.message_id.len());
    let last = held.clone();
    drop((first, second, held, other, hub));
    assert_eq!(root.available_bytes(), 2048);
    assert_eq!(budget.available(), 0);
    drop(last);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert_eq!(budget.available(), bytes);
}

#[test]
fn refusal_rolls_back_both_local_and_class_capacity() {
    let source = fragment("accepted");
    let bytes = copied_bytes(&source, source.delta.len()).unwrap();
    for (local_bytes, data_bytes) in [(bytes - 1, bytes), (bytes, bytes - 1)] {
        let (root, policy) = policy(data_bytes);
        let (budget, _) = setup(local_bytes, &policy);
        assert!(budget.copy_chunk(&source, &source.delta).is_none());
        assert_eq!(root.available_bytes(), root.capacity_bytes());
        assert_eq!(
            policy.budget(IngressClass::Data).available_bytes(),
            data_bytes
        );
        assert_eq!(budget.available(), local_bytes);
    }
    assert!(copied_bytes(&source, usize::MAX).is_none());
}

#[test]
fn multibyte_chunks_keep_one_owner_each_through_broadcast_eviction() {
    let text = "€".repeat(7000);
    let source = fragment(&text);
    let bytes =
        copied_bytes(&source, 16383).unwrap() + copied_bytes(&source, text.len() - 16383).unwrap();
    let (root, policy) = policy(bytes);
    let (budget, hub) = setup(bytes, &policy);
    let mut first = hub.subscribe(&source.agent_id);
    let mut second = hub.subscribe(&source.agent_id);
    hub.publish(Some(source));
    assert_eq!(root.available_bytes(), 2048);
    let mut text_received = String::new();
    for _ in 0..2 {
        let delivery = first.try_recv().unwrap();
        let other = second.try_recv().unwrap();
        assert!(std::ptr::eq(delivery.deref(), other.deref()));
        assert!(delivery.delta.len() <= 16384);
        text_received.push_str(&delivery.delta);
    }
    assert_eq!(text_received, text);
    assert_eq!(budget.available(), bytes);
    for _ in 0..300 {
        hub.publish(Some(fragment("x")));
    }
    assert_eq!(first.try_recv(), Err(TryRecvError::Lagged(44)));
    drop((first, second));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert_eq!(budget.available(), bytes);
}

#[tokio::test]
async fn cancelled_receiver_and_absent_subscribers_do_not_keep_payloads() {
    let source = fragment("after cancellation");
    let bytes = copied_bytes(&source, source.delta.len()).unwrap();
    let (root, policy) = policy(bytes);
    let (budget, hub) = setup(bytes, &policy);
    hub.publish(Some(source.clone()));
    assert_eq!(budget.available(), bytes);
    let mut watching = hub.subscribe(&source.agent_id);
    let mut pending = Box::pin(watching.recv());
    assert!(futures_util::poll!(&mut pending).is_pending());
    drop(pending);
    hub.publish(Some(source));
    assert_eq!(watching.recv().await.unwrap().delta, "after cancellation");
    assert_eq!(budget.available(), bytes);
    drop(watching);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn explicit_policy_uses_data_class_and_retains_the_existing_local_pool() {
    let (_, policy) = policy(4096);
    let admitted = LiveBudget::process_owned(&policy);
    assert!(admitted.pre_admit);
    assert!(Arc::ptr_eq(&admitted.local, &LiveBudget::process().local));
    let held = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("full-data", &[4096]).unwrap(),
        )
        .unwrap();
    let source = fragment("not admitted");
    assert!(admitted.copy_chunk(&source, &source.delta).is_none());
    assert!(!LiveBudget::process().pre_admit);
    assert!(!LiveBudget::process_coordinated().pre_admit);
    drop(held);
}

#[test]
fn concurrent_publication_cannot_overshoot_the_data_class() {
    let source = fragment("concurrent");
    let bytes = copied_bytes(&source, source.delta.len()).unwrap();
    let (root, policy) = policy(bytes * 2);
    let (budget, _) = setup(bytes * 16, &policy);
    let deliveries = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| scope.spawn(|| budget.copy_chunk(&source, &source.delta)))
            .collect();
        handles
            .into_iter()
            .filter_map(|task| task.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(deliveries.len(), 2);
    assert_eq!(root.available_bytes(), 2048);
    assert_eq!(budget.available(), bytes * 14);
    drop(deliveries);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert_eq!(budget.available(), bytes * 16);
}

#[tokio::test]
async fn retained_payload_does_not_restore_a_revoked_publishers_authority() {
    let source = fragment("owned payload");
    let bytes = copied_bytes(&source, source.delta.len()).unwrap();
    let (root, policy) = policy(bytes);
    let (_, hub) = setup(bytes, &policy);
    let mut watching = hub.subscribe(&source.agent_id);
    let authority = crate::agent_ownership::Authority::new(
        crate::agent_ownership::OwnershipToken {
            protocol: crate::agent_ownership::PROTOCOL_VERSION,
            agent_id: source.agent_id.clone(),
            instance_id: "synthetic-owner".into(),
            epoch: 1,
        },
        tokio::time::Instant::now() + std::time::Duration::from_secs(10),
    );
    authority
        .run(async {
            hub.publish(Some(source));
        })
        .await
        .unwrap();
    assert_eq!(root.available_bytes(), 2048);
    authority.revoke();
    assert_eq!(watching.try_recv(), Err(TryRecvError::Lagged(1)));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
