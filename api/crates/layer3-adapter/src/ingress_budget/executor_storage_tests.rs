//! Negative controls distinguishing future/output custody from executor storage.

use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use std::{
    future::poll_fn,
    sync::Mutex,
    task::{Poll, Waker},
};

#[derive(Clone, Copy, Debug)]
enum End {
    Complete,
    Cancel,
    Panic,
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(64 * 1024);
    let floor = AllocationPlan::checked("executor control floor", &[8192]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    drop(policy.admit(IngressClass::Data, floor).unwrap());
    (root, policy)
}

#[test]
fn isolated_future_custody_control_does_not_own_task_storage_kept_by_wakers() {
    const TEST: &str = "ingress_budget::executor_storage_tests::isolated_future_custody_control_does_not_own_task_storage_kept_by_wakers";
    if deallocation::isolated(TEST) {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // Tokio 1.53.3 uses four task-list shards for a current-thread runtime.
    // Initialize their mutex backing outside the per-task storage measurement.
    runtime.block_on(async {
        for _ in 0..4 {
            tokio::spawn(async {}).await.unwrap();
        }
    });
    runtime.block_on(async {
        for end in [End::Complete, End::Cancel, End::Panic] {
            let (root, policy) = policy();
            let retained = Arc::new(Mutex::new(None::<Waker>));
            drop(retained.lock().unwrap());
            let output = retained.clone();
            let scope = IngressWorkScope::new(
                policy
                    .admit(
                        IngressClass::Data,
                        AllocationPlan::checked("future custody control", &[4096]).unwrap(),
                    )
                    .unwrap(),
            );
            let future = scope.initialize_owned(|| {
                poll_fn(move |cx| {
                    *output.lock().unwrap() = Some(cx.waker().clone());
                    match end {
                        End::Complete => Poll::Ready(()),
                        End::Cancel => Poll::Pending,
                        End::Panic => panic!("synthetic executor custody panic"),
                    }
                })
            });
            let mut probe = AllocationProbe::new();
            let task = probe.track(|| tokio::spawn(future.resolve()));
            drop(scope);
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while retained.lock().unwrap().is_none() {
                    tokio::task::yield_now().await;
                }
                if matches!(end, End::Cancel) {
                    task.abort();
                }
                while !task.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let waker = retained.lock().unwrap().take().unwrap();
            if matches!(end, End::Complete) {
                assert_eq!(root.reserved_work_bytes(), 4096);
            }
            let result = task.await;
            match (&result, end) {
                (Ok(_), End::Complete) => (),
                (Err(error), End::Cancel) => assert!(error.is_cancelled()),
                (Err(error), End::Panic) => assert!(error.is_panic()),
                _ => panic!("unexpected executor outcome"),
            }
            drop(result);
            root.assert_idle();
            let executor_bytes = probe.live();
            assert!(
                executor_bytes > 0,
                "the retained task waker must own storage"
            );
            let data_bytes = policy.bounds().data_bytes();
            let pressure = policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked("reused task storage control", &[data_bytes]).unwrap(),
                )
                .unwrap()
                .initialize(|| vec![0u8; data_bytes - sizing::shared_owner_bytes::<Vec<u8>>()]);
            assert!(!pressure.is_empty());
            waker.wake_by_ref();
            assert_eq!(probe.live(), executor_bytes);
            drop(waker);
            assert_eq!(probe.live(), 0, "the final waker frees executor storage");
            drop(pressure);
            eprintln!("{end:?}: {executor_bytes} executor bytes outlived future/result custody");
        }
    });
}
