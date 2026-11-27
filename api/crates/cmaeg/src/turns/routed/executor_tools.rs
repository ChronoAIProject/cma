//! Keep the parent generation while a concurrent tool publishes its receipt.
use super::*;

impl RoutedTargetExecutor {
    pub(super) async fn wait_for_tool(
        &self,
        work: &WorkItem,
        journal: &Journal,
        lease: &mut Lease,
    ) -> Result<ToolReplyStatus, ExecutionError> {
        let bound = Instant::now() + self.invocation_bound;
        let mut renew_at = Instant::now();
        loop {
            let (status, until) = self
                .delivery
                .retained_reply_observation(&work.identity, &work.target_id)
                .await?;
            if status != ToolReplyStatus::Pending {
                return Ok(status);
            }
            let until = until.ok_or(ExecutionError::Busy)?;
            if Instant::now() >= bound {
                return Err(ExecutionError::Busy);
            }
            if Instant::now() >= renew_at {
                journal.renew(lease).await?;
                renew_at = Instant::now() + self.renew_interval;
            }
            // The next read re-evaluates expiry or an exact-claim extension.
            // No single wait can extend beyond the observed guard deadline.
            let remaining = until.saturating_sub((journal.clock)()).max(0) as u64;
            tokio::time::sleep(Duration::from_millis(100).min(Duration::from_secs(remaining)))
                .await;
        }
    }

    pub(super) async fn deliver_with_tool_wait(
        &self,
        work: &WorkItem,
        journal: &Journal,
        lease: &mut Lease,
        frame: &crate::providers::RecoveryEvent,
        effect: super::super::ReplayEffect,
        primary: bool,
    ) -> Result<FrameDelivery, ExecutionError> {
        loop {
            let result = self
                .delivery
                .deliver_frame(
                    &work.identity,
                    &work.target_id,
                    lease,
                    frame,
                    effect.clone(),
                )
                .await;
            if journal.partitions.is_none() || !matches!(result, Err(ExecutionError::Busy)) {
                return result;
            }
            let (status, _) = self
                .delivery
                .retained_reply_observation(&work.identity, &work.target_id)
                .await?;
            if status != ToolReplyStatus::Pending {
                return result;
            }
            if self.wait_for_tool(work, journal, lease).await? == ToolReplyStatus::Unknown
                && !primary
            {
                return Ok(FrameDelivery::Unknown);
            }
            // Retry the same retained effect. The shared guard suppresses it if
            // the tool won; projection is not rerun and no second answer is sent.
        }
    }
}
