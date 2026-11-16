//! Keep the captured journal generation live during the one provider attempt.
use super::{ExecutionError, Journal, Lease, StartResult};
use futures_util::future::BoxFuture;
use std::time::Duration;

impl Journal {
    pub(super) async fn await_acceptance(
        &self,
        lease: &mut Lease,
        mut attempt: BoxFuture<'_, StartResult>,
    ) -> Result<StartResult, ExecutionError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let cadence = crate::replicas::config::RENEW;
        let mut renewal = tokio::time::interval_at(tokio::time::Instant::now() + cadence, cadence);
        renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let result = tokio::time::timeout_at(deadline, async {
            loop {
                tokio::select! {
                    // A renewal never adopts a different claim/epoch. If it
                    // fails, dropping the attempt cannot imply nonacceptance.
                    biased;
                    _ = renewal.tick() => self.renew(lease).await?,
                    result = &mut attempt => {
                        self.guard(lease).await?;
                        return Ok(result);
                    }
                }
            }
        })
        .await;
        match result {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!(
                    target_sha256 = super::storage::hash(lease.state.id.as_bytes()),
                    reason = "acceptance_deadline",
                    "Provider acceptance remains uncertain"
                );
                // The caller must still fence publication of Unknown. It may
                // not publish even this disposition through expired authority.
                Ok(StartResult::Unknown)
            }
        }
    }
}
