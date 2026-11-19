use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{ClassBounds, IngressBudget};

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(128 * 1024);
    let floor = AllocationPlan::checked("watch fixture", &[16 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    drop(policy.admit(IngressClass::Data, floor).unwrap());
    (root, policy)
}

#[tokio::test]
async fn watches_preserve_versions_and_storage_after_sender_teardown() {
    let (root, policy) = policy();
    let (sender, mut receiver) =
        AdmittedWatchSender::channel(&policy, IngressClass::Data, 0u64).unwrap();
    let charged = sender.requested_bytes();
    assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
    assert!(!receiver.has_changed().unwrap());
    assert_eq!(sender.latest(), 0);
    let other_sender = sender.clone();
    let mut other_receiver = receiver.clone();
    let mut subscribed = sender.subscribe();
    assert_eq!(sender.send_replace(4), 0);
    receiver.changed().await.unwrap();
    assert_eq!(receiver.take_latest(), 4);
    assert!(!receiver.has_changed().unwrap());
    other_receiver.changed().await.unwrap();
    assert_eq!(other_receiver.latest(), 4);
    drop((sender, other_sender));
    // A final unseen value remains deliverable even after all senders close.
    subscribed.changed().await.unwrap();
    assert_eq!(subscribed.take_latest(), 4);
    assert!(subscribed.changed().await.is_err());
    drop((receiver, other_receiver));
    assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
    assert_eq!(subscribed.latest(), 4);
    drop(subscribed);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn blocked_and_completed_tasks_retain_watch_storage_until_actual_drop() {
    let (root, policy) = policy();
    let (sender, mut receiver) =
        AdmittedWatchSender::channel(&policy, IngressClass::Data, None::<tokio::time::Instant>)
            .unwrap();
    let held = root.available_bytes();
    let (started, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        started.send(()).unwrap();
        receiver.changed().await.unwrap();
        receiver
    });
    ready.await.unwrap();
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("watch pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    sender.send_replace(Some(tokio::time::Instant::now()));
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop((sender, pressure));
    assert_eq!(root.available_bytes(), held);
    let mut receiver = task.await.unwrap();
    assert!(receiver.take_latest().is_some());
    assert!(receiver.changed().await.is_err());
    drop(receiver);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    let (sender, mut receiver) =
        AdmittedWatchSender::channel(&policy, IngressClass::Data, 0u64).unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        started.send(()).unwrap();
        receiver.changed().await
    });
    ready.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(sender.is_closed());
    sender.closed().await;
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(sender);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn isolated_watch_allocation_and_receiver_lifetime_contract() {
    const TEST: &str =
        "ingress_budget::watch::tests::isolated_watch_allocation_and_receiver_lifetime_contract";
    if std::env::var_os("CMA_WATCH_ALLOCATION_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_WATCH_ALLOCATION_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    measure(0u64, 3u64);
    measure(None, Some(tokio::time::Instant::now()));
    measure((), ());
    measure([0u8; 4096], [1u8; 4096]);
    #[repr(align(256))]
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Aligned([u8; 1024]);
    measure(Aligned([0; 1024]), Aligned([1; 1024]));
}

fn measure<T: Copy + std::fmt::Debug + PartialEq>(initial: T, next: T) {
    let (root, policy) = policy();
    let plan = AdmittedWatchSender::<T>::allocation_plan().unwrap();
    // Negative control: a reservation held only by the source cannot cover an
    // external raw receiver. The real watch allocation outlives that allowance.
    let storage = policy.admit(IngressClass::Data, plan).unwrap();
    let mut probe = AllocationProbe::new();
    let (sender, receiver) = probe.track(|| watch::channel(initial));
    drop((sender, storage));
    assert!(probe.live() > 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    eprintln!(
        "raw_watch_after_sender_drop requested_live={}",
        probe.live()
    );
    drop(receiver);
    assert_eq!(probe.live(), 0);
    drop(probe);
    let mut probe = AllocationProbe::new();
    let (sender, receiver) = probe
        .track(|| AdmittedWatchSender::channel(&policy, IngressClass::Data, initial))
        .unwrap();
    let retained = probe.live();
    assert!(retained > 0);
    assert!(probe.peak() <= plan.requested_bytes());
    let before = probe.allocated();
    let remaining = probe.track(|| {
        let sender_clone = sender.clone();
        let receiver_clone = receiver.clone();
        let mut subscribed = sender.subscribe();
        assert!(!subscribed.has_changed().unwrap());
        assert_eq!(sender_clone.send_replace(next), initial);
        assert_eq!(subscribed.take_latest(), next);
        drop((sender_clone, receiver_clone, sender, receiver));
        subscribed
    });
    assert_eq!(
        probe.allocated(),
        before,
        "published watches must not lazily allocate locks"
    );
    assert_eq!(probe.live(), retained);
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        plan.requested_bytes()
    );
    eprintln!(
        "admitted_watch requested_peak={} retained={} admitted={}",
        probe.peak(),
        probe.live(),
        plan.requested_bytes()
    );
    probe.track(|| drop(remaining));
    assert_eq!(probe.allocated(), before);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    drop(probe);
    let available = policy.budget(IngressClass::Data).available_bytes();
    let blocker = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("watch pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| AdmittedWatchSender::channel(&policy, IngressClass::Data, initial));
    assert!(matches!(result, Err(AdmissionRefusal::AtCapacity { .. })));
    assert_eq!(probe.allocated(), 0);
    drop((result, blocker));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
