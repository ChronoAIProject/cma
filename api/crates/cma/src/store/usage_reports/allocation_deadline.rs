//! Allocation cursors have a finite wall budget in addition to Mongo maxTimeMS.
use super::*;
use std::future::Future;

pub(in crate::store) const ALLOCATION_DB_DEADLINE: Duration = Duration::from_secs(5);
const ALLOCATION_OPERATION_DEADLINE: Duration = Duration::from_secs(10);
// Pool-wide hold protection allows one bounded allocation operation plus slack.
// Checkout, connection establishment, and other Mongo operations retain 2 s.
pub(super) const REPORTING_HOLD_DEADLINE: Duration = Duration::from_secs(6);

pub(in crate::store) async fn allocation_operation<T>(
    operation: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(ALLOCATION_OPERATION_DEADLINE, operation)
        .await
        .map_err(|_| unavailable("allocation operation deadline exceeded"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test(start_paused = true)]
    async fn allocation_interval_stage_deadline_drops_incomplete_work() {
        struct DropMark(Arc<AtomicBool>);
        impl Drop for DropMark {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let began = tokio::time::Instant::now();
        let result: Result<()> = allocation_operation(async {
            let _mark = DropMark(dropped.clone());
            std::future::pending().await
        })
        .await;
        assert!(matches!(result, Err(MetricsError::Unavailable)));
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(began.elapsed(), ALLOCATION_OPERATION_DEADLINE);
    }

    #[tokio::test(start_paused = true)]
    async fn allocation_interval_stage_accepts_bounded_work_and_preserves_errors() {
        assert_eq!(
            allocation_operation(async {
                tokio::time::sleep(ALLOCATION_DB_DEADLINE).await;
                Ok(17)
            })
            .await
            .unwrap(),
            17
        );
        assert!(matches!(
            allocation_operation(async { Err::<(), _>(MetricsError::Forbidden) }).await,
            Err(MetricsError::Forbidden)
        ));
        let mut options = ClientOptions::default();
        configure_pool(&mut options, 4);
        assert_eq!(options.connect_timeout, Some(Duration::from_secs(2)));
        assert_eq!(
            options.server_selection_timeout,
            Some(Duration::from_secs(2))
        );
    }
}
