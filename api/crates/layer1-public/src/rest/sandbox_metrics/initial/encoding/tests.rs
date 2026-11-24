use super::*;
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[test]
fn initial_body_limit_counts_final_escaped_utf8_bytes_at_boundary() {
    let value = json!({"name":"\"\n雪".repeat(64),"quantity":9_007_199_254_740_992_f64});
    let expected = serde_json::to_vec(&wire::value(&value)).unwrap();
    for limit in [expected.len() - 1, expected.len(), expected.len() + 1] {
        let result = encode(&value, limit, Instant::now() + Duration::from_secs(5));
        if limit < expected.len() {
            assert!(matches!(result, Err(MetricsError::Unavailable)));
        } else {
            assert_eq!(result.unwrap(), expected);
        }
    }
}

struct Slow;
impl serde::Serialize for Slow {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        std::thread::sleep(Duration::from_millis(10));
        serializer.serialize_str("must not escape the deadline")
    }
}
#[test]
fn initial_deadline_includes_synchronous_wire_projection_and_final_serialization() {
    assert!(matches!(
        encode(&Slow, 1024, Instant::now() + Duration::from_millis(2)),
        Err(MetricsError::Unavailable)
    ));
    assert!(encode(&json!({}), 1024, Instant::now()).is_err());
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
#[tokio::test(start_paused = true)]
async fn initial_outer_timeout_drops_work_and_emits_only_typed_problem() {
    let dropped = Arc::new(AtomicBool::new(false));
    let response = respond(async {
        let _guard = Dropped(dropped.clone());
        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok((false, json!({"partial":"must not escape"})))
    })
    .await;
    assert_eq!(response.status(), 503);
    assert!(dropped.load(Ordering::SeqCst));
    let bytes = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    assert!(!std::str::from_utf8(&bytes).unwrap().contains("partial"));
}

#[tokio::test]
async fn initial_cancellation_drops_work_without_detached_tasks() {
    let dropped = Arc::new(AtomicBool::new(false));
    {
        let mut future = Box::pin(respond(async {
            let _guard = Dropped(dropped.clone());
            std::future::pending::<()>().await;
            Ok((false, json!({})))
        }));
        assert!(futures_util::poll!(future.as_mut()).is_pending());
    }
    assert!(dropped.load(Ordering::SeqCst));
}
