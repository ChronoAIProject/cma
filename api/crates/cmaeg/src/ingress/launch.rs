//! Contract between verified ingress and provider turn execution.
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::Activity,
    ids::{ConsumerId, ConversationId, IdempotencyKey},
    problem::{Problem, ProblemCode},
    seal::EventRef,
};
use futures_util::future::BoxFuture;
use std::{sync::Arc, time::Instant};
use tokio::sync::OwnedSemaphorePermit;

#[derive(Debug)]
pub struct LaunchRequest {
    pub channel: Arc<ResolvedChannel>,
    pub activity: Activity,
    pub conversation_id: ConversationId,
    pub event_ref: EventRef,
    pub idempotency_key: IdempotencyKey,
    pub received_at: Instant,
    pub trace: tracing::Span,
    pub timing: Option<crate::telemetry::timing::IngressTiming>,
    /// The consumer that serves this turn. None means the default consumer.
    pub consumer: Option<ConsumerId>,
}
#[derive(Debug)]
pub enum LaunchOutcome {
    /// Committed at the gateway; provider execution is recovered separately.
    Admitted {
        admission_id: String,
        conversation_id: ConversationId,
        /// Only this request's exact waiter is retained by the primary target.
        /// Duplicate HTTP streams must not attach to a different/new waiter.
        sync_waiter_attached: bool,
    },
    Accepted {
        response_id: String,
    },
    Deferred,
    Refused(Problem),
}
pub trait TurnLauncher: Send + Sync {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome>;
    /// The permit bounds shared ingress capacity. The default holds it until
    /// `launch` completes; implementations may release it earlier but must
    /// keep their acceptance semantics.
    fn launch_holding(
        &self,
        request: LaunchRequest,
        hold: OwnedSemaphorePermit,
    ) -> BoxFuture<'_, LaunchOutcome> {
        Box::pin(async move {
            let outcome = self.launch(request).await;
            drop(hold);
            outcome
        })
    }
}
/// Explicit refusal for integrations that compose ingress without a turn runner.
pub struct UnavailableLauncher;
impl TurnLauncher for UnavailableLauncher {
    fn launch(&self, _request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        Box::pin(async {
            LaunchOutcome::Refused(Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "turn runner is not configured",
            ))
        })
    }
}
