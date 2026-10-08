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
    /// Multiplex every consumer into Sync; plain streams retain only child zero.
    pub sync_fanout: bool,
    /// The runner takes this at execute entry. Delivery-held request clones
    /// must not keep capacity after execution ends; spawn refusal drops it.
    pub lease: std::sync::Mutex<Option<crate::turns::capacity::TurnLease>>,
}
#[derive(Debug)]
pub enum LaunchOutcome {
    Accepted { response_id: String },
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
