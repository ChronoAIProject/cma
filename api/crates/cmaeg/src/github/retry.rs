//! Retry only definitely aborted/conflicting transactions, within ingress's budget.
use super::Error;
use std::{future::Future, time::Duration};
use tokio::time::{sleep, timeout_at, Instant};

pub(super) async fn run<T, F, Fut>(mut action: F) -> Result<T, Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    let deadline = Instant::now() + Duration::from_millis(3500);
    for attempt in 0..64 {
        match timeout_at(deadline, action()).await {
            Ok(Err(Error::Retry)) => {}
            Ok(result) => return result,
            Err(_) => return Err(Error::Unavailable),
        }
        let cap = (8_u64 << attempt.min(4)).min(128);
        let jitter = 4 + (uuid::Uuid::new_v4().as_u128() as u64 % cap);
        let wake = Instant::now() + Duration::from_millis(jitter);
        if wake >= deadline {
            break;
        }
        sleep(Duration::from_millis(jitter)).await;
    }
    Err(Error::Unavailable)
}
