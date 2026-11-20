//! Bounded majority-readback used after a concurrent immutable write.
use std::{future::Future, time::Duration};

/// Waits before each read; together they stay well inside the two-second deadline.
const DELAYS_MS: [u64; 7] = [0, 5, 10, 20, 40, 80, 160];

/// Retries `read` on a short backoff for at most two seconds. `None` means the
/// value never became visible; a read error or the deadline is returned as is.
pub(crate) async fn bounded<T, E, F, Fut>(
    mut read: F,
    timeout_error: impl Fn() -> E,
) -> Result<Option<T>, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<T>, E>>,
{
    tokio::time::timeout(Duration::from_secs(2), async {
        for delay_ms in DELAYS_MS {
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            if let Some(value) = read().await? {
                return Ok(Some(value));
            }
        }
        Ok(None)
    })
    .await
    .map_err(|_| timeout_error())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn delayed_visibility_returns_the_winner_without_another_write() {
        let mut reads = 0;
        let start = tokio::time::Instant::now();
        let winner = bounded(
            || {
                reads += 1;
                std::future::ready(Ok::<_, ()>((reads == 4).then_some("winner")))
            },
            || (),
        )
        .await
        .unwrap();
        assert_eq!(winner, Some("winner"));
        assert_eq!(reads, 4);
        assert_eq!(start.elapsed(), Duration::from_millis(35));
    }

    #[tokio::test(start_paused = true)]
    async fn absent_readback_is_bounded_and_errors_are_preserved() {
        let mut reads = 0;
        assert_eq!(
            bounded(
                || {
                    reads += 1;
                    std::future::ready(Ok::<Option<()>, &str>(None))
                },
                || "timeout"
            )
            .await,
            Ok(None)
        );
        assert_eq!(reads, 7);
        assert_eq!(
            bounded(
                || std::future::ready(Err::<Option<()>, _>("corrupt")),
                || "timeout"
            )
            .await,
            Err("corrupt")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_read_respects_the_total_two_second_budget() {
        let start = tokio::time::Instant::now();
        assert_eq!(
            bounded(std::future::pending::<Result<Option<()>, &str>>, || {
                "timeout"
            })
            .await,
            Err("timeout")
        );
        assert_eq!(start.elapsed(), Duration::from_secs(2));
    }
}
