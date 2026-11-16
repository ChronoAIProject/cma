//! Bounded terminal observation, independent of the request's HTTP outcome.
use serde_json::{json, Value};
use std::{future::Future, time::Duration};
use tokio::time::{sleep_until, timeout_at, Instant};

pub(super) struct Observation {
    pub durable: Value,
    pub expired: bool,
    pub attempts: u32,
    pub last_successful: Option<Value>,
    pub last_successful_ms: Option<u128>,
}

pub(super) async fn observe<F, O>(budget: Duration, mut read: F) -> Observation
where
    F: FnMut() -> O,
    O: Future<Output = Result<Value, &'static str>>,
{
    let started = Instant::now();
    let deadline = started + budget;
    let mut result = Observation {
        durable: json!({"oracle_error":"not_observed"}),
        expired: true,
        attempts: 0,
        last_successful: None,
        last_successful_ms: None,
    };
    loop {
        // Sleep may have consumed the remaining budget. A new expired read
        // would erase the last observation without learning anything.
        if Instant::now() >= deadline {
            return result;
        }
        result.attempts += 1;
        let observation = timeout_at(
            deadline.min(Instant::now() + Duration::from_secs(3)),
            read(),
        )
        .await;
        result.durable = match observation {
            Ok(Ok(value)) => {
                result.last_successful = Some(value.clone());
                result.last_successful_ms = Some(started.elapsed().as_millis());
                value
            }
            Ok(Err(_)) => json!({"oracle_error":"read_failed"}),
            Err(_) => json!({"oracle_error":"read_timeout"}),
        };
        let complete = result.durable["targets"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty() && rows.iter().all(|row| row["complete"] == true));
        if Instant::now() >= deadline {
            return result;
        }
        if complete {
            result.expired = false;
            return result;
        }
        sleep_until(deadline.min(Instant::now() + Duration::from_millis(100))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test(start_paused = true)]
    async fn expired_poll_does_not_overwrite_successful_absence() {
        let calls = AtomicUsize::new(0);
        let absent = json!({"root_present":false,"targets":[]});
        let started = Instant::now();
        let result = observe(Duration::from_millis(100), || {
            let number = calls.fetch_add(1, Ordering::SeqCst);
            let value = absent.clone();
            async move {
                if number > 0 {
                    // A real Mongo read yields; an already-expired poll would
                    // replace the earlier success with a synthetic timeout.
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                Ok(value)
            }
        })
        .await;
        assert!(result.expired);
        assert_eq!(result.durable, absent);
        assert_eq!(result.last_successful, Some(absent));
        assert_eq!(result.attempts, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(started.elapsed(), Duration::from_millis(100));
    }

    #[tokio::test(start_paused = true)]
    async fn real_read_timeout_remains_incomplete() {
        let started = Instant::now();
        let result = observe(Duration::from_millis(100), || async {
            std::future::pending::<Result<Value, &'static str>>().await
        })
        .await;
        assert!(result.expired);
        assert_eq!(result.durable, json!({"oracle_error":"read_timeout"}));
        assert!(result.last_successful.is_none());
        assert_eq!(started.elapsed(), Duration::from_millis(100));
    }

    #[tokio::test(start_paused = true)]
    async fn real_read_failure_is_retained() {
        let result = observe(Duration::from_millis(100), || async {
            Err("oracle plan read")
        })
        .await;
        assert!(result.expired);
        assert_eq!(result.durable, json!({"oracle_error":"read_failed"}));
        assert!(result.last_successful.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn completion_requires_every_target_and_returns_without_extra_read() {
        let calls = AtomicUsize::new(0);
        let result = observe(Duration::from_secs(1), || {
            let number = calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(json!({"targets":[{"complete":true},{"complete":number > 0}]})) }
        })
        .await;
        assert!(!result.expired);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(result.durable["targets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["complete"] == true));
    }

    #[tokio::test(start_paused = true)]
    async fn later_real_timeout_keeps_earlier_success_separately() {
        let calls = AtomicUsize::new(0);
        let absent = json!({"root_present":false,"targets":[]});
        let result = observe(Duration::from_millis(200), || {
            let number = calls.fetch_add(1, Ordering::SeqCst);
            let value = absent.clone();
            async move {
                if number > 0 {
                    std::future::pending::<()>().await;
                }
                Ok(value)
            }
        })
        .await;
        assert!(result.expired);
        assert_eq!(result.durable, json!({"oracle_error":"read_timeout"}));
        assert_eq!(result.last_successful, Some(absent));
        assert_eq!(result.last_successful_ms, Some(0));
        assert_eq!(result.attempts, 2);
    }
}
