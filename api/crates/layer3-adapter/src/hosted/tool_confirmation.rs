//! Keep a completed native open alive while its exact durable confirmation retries.
use std::future::Future;
use std::time::Duration;

use crate::AdapterError;

const ATTEMPTS: usize = 3;
const ATTEMPT_DEADLINE: Duration = Duration::from_secs(5);
const TOTAL_DEADLINE: Duration = Duration::from_secs(15);

/// Retry only the original confirmation. The source must revalidate authority
/// and accept an exact already-committed receipt after an ambiguous response.
/// No native open, descriptor resolution, prompt or publication belongs here.
pub(super) async fn confirm<F, Fut>(mut operation: F) -> crate::Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = crate::Result<()>>,
{
    tokio::time::timeout(TOTAL_DEADLINE, async {
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(50 << (attempt - 1))).await;
            }
            let result = tokio::time::timeout(ATTEMPT_DEADLINE, operation())
                .await
                .unwrap_or_else(|_| Err(timed_out()));
            match result {
                Err(AdapterError::Unavailable(_)) if attempt + 1 < ATTEMPTS => {
                    tracing::warn!(
                        attempt = attempt + 1,
                        "Retrying completed Agent tool confirmation"
                    );
                }
                other => return other,
            }
        }
        unreachable!("the last confirmation result is returned")
    })
    .await
    .unwrap_or_else(|_| Err(timed_out()))
}

fn timed_out() -> AdapterError {
    AdapterError::Unavailable("completed Agent tool confirmation timed out".into())
}

#[cfg(test)]
#[path = "tool_confirmation_tests.rs"]
mod tests;
