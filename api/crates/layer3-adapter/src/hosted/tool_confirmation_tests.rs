use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

#[tokio::test(start_paused = true)]
async fn ambiguous_commit_is_read_back_without_replacing_the_completed_open() {
    let calls = AtomicUsize::new(0);
    let committed = std::sync::Mutex::new(None);
    let exact = (
        "runtime-boot",
        "agent-open-request",
        "retained-conversation",
    );
    confirm(|| async {
        let mut receipt = committed.lock().unwrap();
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            *receipt = Some(exact);
            return Err(AdapterError::Unavailable("commit reply lost".into()));
        }
        assert_eq!(*receipt, Some(exact));
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn changed_authority_stops_retry_even_after_a_transient_failure() {
    let calls = AtomicUsize::new(0);
    let result = confirm(|| async {
        match calls.fetch_add(1, Ordering::SeqCst) {
            0 => Err(AdapterError::Unavailable("storage conflict".into())),
            1 => Err(AdapterError::Rejected("runtime boot changed".into())),
            _ => panic!("a refused authority must not retry"),
        }
    })
    .await;
    assert!(
        matches!(result, Err(AdapterError::Rejected(reason)) if reason == "runtime boot changed")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn permanent_failure_is_not_retried_or_converted_to_success() {
    for unsupported in [false, true] {
        let calls = AtomicUsize::new(0);
        let result = confirm(|| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(if unsupported {
                AdapterError::Unsupported("unproved tools".into())
            } else {
                AdapterError::Rejected("wrong conversation".into())
            })
        })
        .await;
        assert!(matches!(
            result,
            Err(AdapterError::Unsupported(_) | AdapterError::Rejected(_))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn repeated_storage_outage_preserves_the_final_failure_and_is_bounded() {
    let calls = AtomicUsize::new(0);
    let result = confirm(|| async {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(AdapterError::Unavailable(
            "storage remains unavailable".into(),
        ))
    })
    .await;
    assert!(
        matches!(result, Err(AdapterError::Unavailable(reason)) if reason == "storage remains unavailable")
    );
    assert_eq!(calls.load(Ordering::SeqCst), ATTEMPTS);
}

#[tokio::test(start_paused = true)]
async fn unanswered_confirmations_share_one_total_deadline() {
    let calls = AtomicUsize::new(0);
    let started = tokio::time::Instant::now();
    let result = confirm(|| async {
        calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<crate::Result<()>>().await
    })
    .await;
    assert!(matches!(result, Err(AdapterError::Unavailable(_))));
    assert_eq!(started.elapsed(), TOTAL_DEADLINE);
    assert_eq!(calls.load(Ordering::SeqCst), ATTEMPTS);
}

#[tokio::test(start_paused = true)]
async fn timeouts_drop_each_waiter_and_cancellation_starts_no_further_attempt() {
    use std::sync::Arc;
    struct Waiter(Arc<AtomicUsize>);
    impl Drop for Waiter {
        fn drop(&mut self) {
            assert_eq!(self.0.fetch_sub(1, Ordering::SeqCst), 1);
        }
    }
    let active = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let worker_active = active.clone();
    let worker_calls = calls.clone();
    let worker = tokio::spawn(async move {
        confirm(|| async {
            assert_eq!(worker_active.fetch_add(1, Ordering::SeqCst), 0);
            let _waiter = Waiter(worker_active.clone());
            worker_calls.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<crate::Result<()>>().await
        })
        .await
    });
    tokio::task::yield_now().await;
    assert_eq!(active.load(Ordering::SeqCst), 1);
    tokio::time::advance(ATTEMPT_DEADLINE).await;
    tokio::task::yield_now().await;
    assert_eq!(active.load(Ordering::SeqCst), 0);
    tokio::time::advance(Duration::from_millis(50)).await;
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(active.load(Ordering::SeqCst), 1);
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert_eq!(active.load(Ordering::SeqCst), 0);
    tokio::time::advance(TOTAL_DEADLINE).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
