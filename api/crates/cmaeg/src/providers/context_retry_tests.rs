use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[tokio::test(start_paused = true)]
async fn timed_out_read_retries_once_after_backoff_within_total_budget() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();
    let result = retry(|| {
        let attempt = attempts.fetch_add(1, Ordering::SeqCst);
        async move {
            if attempt == 0 {
                std::future::pending::<()>().await;
            }
            Ok(None)
        }
    })
    .await;
    assert!(matches!(result, Ok(None)));
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(start.elapsed() >= Duration::from_millis(3500));
    assert!(start.elapsed() <= Duration::from_secs(4));
}

#[tokio::test(start_paused = true)]
async fn persistent_timeout_is_not_absence_and_never_exceeds_two_attempts() {
    let attempts = AtomicUsize::new(0);
    let start = Instant::now();
    let result = retry(|| {
        attempts.fetch_add(1, Ordering::SeqCst);
        std::future::pending()
    })
    .await;
    let failure = result.err().unwrap();
    assert_eq!(failure.class, "timeout");
    assert!(failure.retry_after.is_some());
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(start.elapsed() <= TOTAL_BOUND);
}

#[tokio::test(start_paused = true)]
async fn refusal_and_malformed_results_are_not_retried() {
    for error in [
        ProviderError::Rejected {
            status: 401,
            code: "context_denied".into(),
        },
        ProviderError::Rejected {
            status: 403,
            code: "context_denied".into(),
        },
        ProviderError::ResponseMalformed("invalid context".into()),
        ProviderError::InvalidRequest("identity changed".into()),
    ] {
        let attempts = AtomicUsize::new(0);
        let result = retry(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            let error = error.clone();
            async move { Err(error.into()) }
        })
        .await;
        assert_eq!(result.err().unwrap().error, error);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn retry_after_is_a_minimum_and_is_never_shortened_to_fit() {
    for seconds in [2_u64, 10] {
        let attempts = AtomicUsize::new(0);
        let start = Instant::now();
        let result = retry(|| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt == 0 {
                    Err(ContextFailure::response(429, Some(&seconds.into())))
                } else {
                    Ok(None)
                }
            }
        })
        .await;
        if seconds == 2 {
            assert!(matches!(result, Ok(None)));
            assert!(start.elapsed() >= Duration::from_secs(2));
            assert_eq!(attempts.load(Ordering::SeqCst), 2);
        } else {
            assert_eq!(
                result.err().unwrap().retry_after,
                Some(Duration::from_secs(10))
            );
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
        }
    }
}

#[test]
fn only_transient_statuses_retry_and_date_minima_are_respected() {
    for status in [400, 401, 403, 409, 422] {
        assert!(ContextFailure::response(status, None).retry_after.is_none());
    }
    for status in [429, 500, 502, 503, 504] {
        assert_eq!(
            ContextFailure::response(status, None).retry_after,
            Some(MIN_BACKOFF)
        );
    }
    let date = httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(120));
    let failure = ContextFailure::response(503, Some(&date.parse().unwrap()));
    assert!(failure.retry_after.unwrap() >= Duration::from_secs(119));
    assert!(
        ContextFailure::response(503, Some(&"invalid".parse().unwrap()))
            .retry_after
            .is_none()
    );
}

#[tokio::test(start_paused = true)]
async fn cancellation_drops_pending_attempt_and_backoff_without_detached_reads() {
    for pending in [false, true] {
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = attempts.clone();
        let task = tokio::spawn(async move {
            retry(|| {
                observed.fetch_add(1, Ordering::SeqCst);
                async move {
                    if pending {
                        std::future::pending::<()>().await;
                    }
                    Err(ContextFailure::transient("transport"))
                }
            })
            .await
            .map_err(|_| ())
        });
        tokio::task::yield_now().await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        task.abort();
        let _ = task.await;
        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
