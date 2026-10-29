//! Concrete opt-in scheduler executor. Accepted/uncertain starts are never reposted.
use super::{
    replay_projection::ReplayProjection,
    scheduler::{Disposition, TargetExecutor, WorkItem},
    DispatchOutcome, ExecutionError, FrameDelivery, FrozenTurn, HttpExecutionPort, Journal, Lease,
    ReattachError, RoutedDelivery, ToolReplyStatus,
};
use crate::{turns::tracker::TurnTracker, DeliveryConfig};
use futures_util::future::BoxFuture;
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;
const RETRY: Duration = Duration::from_secs(5);
const RENEW: Duration = Duration::from_secs(10);
const GET_BOUND: Duration = Duration::from_secs(15);
const INVOCATION_BOUND: Duration = Duration::from_secs(60);
const MAX_FRAMES: usize = 4096;
const MAX_RECONNECTS: usize = 3;

pub struct RoutedTargetExecutor {
    port: Arc<HttpExecutionPort>,
    delivery: RoutedDelivery,
    tracker: Arc<TurnTracker>,
    worker: String,
    failure_text: String,
    pub(super) frame_limit: usize,
    pub(super) renew_interval: Duration,
    pub(super) invocation_bound: Duration,
}
impl RoutedTargetExecutor {
    pub fn new(
        port: Arc<HttpExecutionPort>,
        delivery: RoutedDelivery,
        tracker: Arc<TurnTracker>,
        config: &DeliveryConfig,
    ) -> Self {
        Self {
            port,
            delivery,
            tracker,
            worker: format!("routed-{}", uuid::Uuid::new_v4()),
            failure_text: config.failure_text.clone(),
            frame_limit: MAX_FRAMES,
            renew_interval: RENEW,
            invocation_bound: INVOCATION_BOUND,
        }
    }
    async fn run(&self, work: &WorkItem) -> Result<Disposition, ExecutionError> {
        let dispatcher = self.port.clone().dispatcher();
        let journal = dispatcher.journal();
        let outcome = dispatcher
            .dispatch(&work.identity, &work.target_id, &self.worker)
            .await?;
        let DispatchOutcome::Replay {
            mut lease,
            body,
            response_id,
            durable_cursor,
        } = outcome
        else {
            return Ok(match outcome {
                DispatchOutcome::Retry => Disposition::Retry(RETRY),
                DispatchOutcome::Terminal(super::Phase::Refused { code })
                    if code == super::expiry::EXPIRED_BEFORE_START =>
                {
                    self.delivery
                        .report_expired_before_start(&work.identity, &work.target_id)
                        .await?;
                    Disposition::Settled
                }
                _ => Disposition::Settled,
            });
        };
        let result = self
            .replay(
                work,
                journal,
                &mut lease,
                &body,
                response_id,
                durable_cursor,
            )
            .await;
        // A failed release leaves the bounded lease as the recovery authority.
        // In particular, never attempt to overwrite a newer owner's record.
        let _ = journal.release(&mut lease).await;
        result
    }
    async fn replay(
        &self,
        work: &WorkItem,
        journal: &Journal,
        lease: &mut Lease,
        body: &FrozenTurn,
        response_id: String,
        durable_cursor: Option<u64>,
    ) -> Result<Disposition, ExecutionError> {
        journal.renew(lease).await?;
        let mut projection = match tokio::time::timeout(GET_BOUND, journal.load_projection(lease))
            .await
        {
            Ok(Ok(Some(projection))) => projection,
            Ok(Ok(None)) if durable_cursor.is_none() => ReplayProjection::new(response_id.clone()),
            Ok(Ok(None)) => return Err(ExecutionError::Corrupt),
            Ok(Err(error)) => return Err(error),
            Err(_) => return Ok(Disposition::Retry(RETRY)),
        };
        if projection.terminal_acknowledged(lease) {
            journal.complete(lease, &response_id).await?;
            journal
                .observe(
                    &work.identity,
                    crate::activity::Change::settled(
                        body.delivery.consumer.as_str(),
                        response_id.clone(),
                    ),
                )
                .await;
            return Ok(Disposition::Settled);
        }
        if lease.state.delivery.as_ref().is_some_and(|v| {
            matches!(
                v.status,
                super::DeliveryStatus::Unknown | super::DeliveryStatus::Partial { .. }
            )
        }) {
            return Ok(Disposition::Settled);
        }
        let started = Instant::now();
        let mut observed = 0;
        for _ in 0..MAX_RECONNECTS {
            if started.elapsed() >= self.invocation_bound {
                break;
            }
            journal.renew(lease).await?;
            let mut attached = match tokio::time::timeout(
                GET_BOUND,
                self.port.reattach(
                    &work.identity,
                    &work.target_id,
                    lease,
                    projection.native_agent_id.as_deref(),
                ),
            )
            .await
            {
                Ok(Ok(attached)) => attached,
                Ok(Err(ReattachError::Execution(error))) => return Err(error),
                Ok(Err(ReattachError::Provider(_))) | Err(_) => continue,
            };
            journal.renew(lease).await?;
            if projection.native_agent_id.is_none() {
                projection.native_agent_id = Some(attached.snapshot.native_agent_id);
                journal.save_projection(lease, &projection).await?;
                let mut change = crate::activity::Change::state(
                    body.delivery.consumer.as_str(),
                    crate::activity::ConsumerState::Accepted,
                    Some(response_id.clone()),
                );
                change.agent_id = projection.native_agent_id.clone();
                journal.observe(&work.identity, change).await;
            }
            loop {
                if started.elapsed() >= self.invocation_bound || observed >= self.frame_limit {
                    return Ok(Disposition::Retry(RETRY));
                }
                let frame =
                    match tokio::time::timeout(self.renew_interval, attached.stream.events.recv())
                        .await
                    {
                        Ok(Some(Ok(frame))) => frame,
                        Ok(Some(Err(_))) | Ok(None) => break,
                        Err(_) => {
                            journal.renew(lease).await?;
                            continue;
                        }
                    };
                observed += 1;
                journal.renew(lease).await?;
                let effect = if let Some(effect) = projection.pending_effect(&frame)? {
                    effect
                } else {
                    let durable_reply = if projection.needs_tool_receipt(&frame, body) {
                        match self
                            .delivery
                            .tool_reply_status(&work.identity, &work.target_id, lease, body)
                            .await?
                        {
                            ToolReplyStatus::None => false,
                            ToolReplyStatus::Confirmed => true,
                            ToolReplyStatus::Pending => return Ok(Disposition::Retry(RETRY)),
                            ToolReplyStatus::Unknown => return Ok(Disposition::Settled),
                        }
                    } else {
                        false
                    };
                    let tool_replied = durable_reply
                        || self.tracker.has_tool_reply(
                            body.event_context.event_ref.as_str(),
                            (journal.clock)(),
                        );
                    let effect =
                        projection.project(&frame, body, tool_replied, &self.failure_text)?;
                    journal.save_projection(lease, &projection).await?;
                    effect
                };
                match self
                    .delivery
                    .deliver_frame(&work.identity, &work.target_id, lease, &frame, effect)
                    .await?
                {
                    FrameDelivery::Unknown => return Ok(Disposition::Settled),
                    FrameDelivery::Settled { .. } => {}
                }
                if projection.terminal_acknowledged(lease) {
                    journal.complete(lease, &response_id).await?;
                    journal
                        .observe(
                            &work.identity,
                            crate::activity::Change::settled(
                                body.delivery.consumer.as_str(),
                                response_id.clone(),
                            ),
                        )
                        .await;
                    return Ok(Disposition::Settled);
                }
            }
            // EOF, malformed/truncated replay, or observation errors retain the
            // accepted response and committed cursor. Reconnect uses GET only.
        }
        Ok(Disposition::Retry(RETRY))
    }
}
impl TargetExecutor for RoutedTargetExecutor {
    fn execute<'a>(&'a self, work: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async move {
            match self.run(work).await {
                Ok(result) => result,
                Err(error) => {
                    tracing::warn!(target_id = %work.target_id, ?error, "routed execution deferred");
                    match error {
                        ExecutionError::Busy
                        | ExecutionError::LostLease
                        | ExecutionError::Unavailable
                        | ExecutionError::Uncertain => Disposition::Retry(RETRY),
                        _ => Disposition::Settled,
                    }
                }
            }
        })
    }
}
