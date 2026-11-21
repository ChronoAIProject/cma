use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Barrier,
};

#[test]
fn components_share_capacity_and_both_refusal_paths_rollback() {
    let aggregate = IngressBudget::new(10);
    let raw = Arc::new(Semaphore::new(20));
    let parsed = Arc::new(Semaphore::new(4));
    let held = IngressReservation::try_acquire(&raw, Some(&aggregate), 8).unwrap();
    assert!(IngressReservation::try_acquire(&parsed, Some(&aggregate), 3).is_none());
    assert_eq!(parsed.available_permits(), 4);
    assert_eq!(aggregate.available_bytes(), 2);
    assert!(IngressReservation::try_acquire(&parsed, Some(&aggregate), 5).is_none());
    assert_eq!(aggregate.available_bytes(), 2);
    assert!(IngressReservation::try_acquire(&raw, Some(&aggregate), usize::MAX).is_none());
    assert_eq!(raw.available_permits(), 12);
    drop(held);
    assert_eq!(aggregate.available_bytes(), 10);
    assert_eq!(raw.available_permits(), 20);
    let accepted = IngressReservation::try_acquire(&parsed, Some(&aggregate), 4).unwrap();
    drop(accepted);
    assert_eq!(parsed.available_permits(), 4);
    assert_eq!(aggregate.available_bytes(), 10);
}

#[test]
fn concurrent_components_never_oversubscribe_and_recover_every_permit() {
    let aggregate = IngressBudget::new(8);
    let components = [Arc::new(Semaphore::new(8)), Arc::new(Semaphore::new(8))];
    let admitted = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(17));
    let mut workers = Vec::new();
    for index in 0..16 {
        let aggregate = aggregate.clone();
        let component = components[index % 2].clone();
        let admitted = admitted.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let held = IngressReservation::try_acquire(&component, Some(&aggregate), 1);
            if held.is_some() {
                admitted.fetch_add(1, Ordering::SeqCst);
            }
            barrier.wait();
            barrier.wait();
            drop(held);
        }));
    }
    barrier.wait();
    assert_eq!(admitted.load(Ordering::SeqCst), 8);
    assert_eq!(aggregate.available_bytes(), 0);
    assert_eq!(
        components
            .iter()
            .map(|c| c.available_permits())
            .sum::<usize>(),
        8
    );
    barrier.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(aggregate.available_bytes(), 8);
    assert!(components.iter().all(|c| c.available_permits() == 8));
}

#[test]
fn final_storage_owner_releases_before_either_reservation() {
    struct Storage(IngressBudget, Arc<Semaphore>);
    impl Drop for Storage {
        fn drop(&mut self) {
            assert_eq!(self.0.available_bytes(), 0);
            assert_eq!(self.1.available_permits(), 0);
        }
    }
    struct Owned {
        _storage: Storage,
        _lease: IngressReservation,
    }
    let aggregate = IngressBudget::new(8);
    let component = Arc::new(Semaphore::new(8));
    let value = Arc::new(Owned {
        _storage: Storage(aggregate.clone(), component.clone()),
        _lease: IngressReservation::try_acquire(&component, Some(&aggregate), 8).unwrap(),
    });
    let last = value.clone();
    drop(value);
    assert_eq!(aggregate.available_bytes(), 0);
    drop(last);
    assert_eq!(aggregate.available_bytes(), 8);
    assert_eq!(component.available_permits(), 8);
}

#[tokio::test]
async fn cancellation_returns_both_components_without_a_waiting_upgrade() {
    let aggregate = IngressBudget::new(8);
    let component = Arc::new(Semaphore::new(8));
    let (ready, started) = tokio::sync::oneshot::channel();
    let pool = aggregate.clone();
    let local = component.clone();
    let task = tokio::spawn(async move {
        let _held = IngressReservation::try_acquire(&local, Some(&pool), 8).unwrap();
        ready.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    started.await.unwrap();
    assert!(IngressReservation::try_acquire(&component, Some(&aggregate), 1).is_none());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(aggregate.available_bytes(), 8);
    assert_eq!(component.available_permits(), 8);
}

#[test]
fn disabled_coordinator_preserves_component_admission_and_process_pool_is_shared() {
    let component = Arc::new(Semaphore::new(16));
    let held = IngressReservation::try_acquire(&component, None, 16).unwrap();
    assert_eq!(component.available_permits(), 0);
    drop(held);
    assert_eq!(component.available_permits(), 16);
    assert!(Arc::ptr_eq(
        &IngressBudget::process().0,
        &IngressBudget::process().0
    ));
}
