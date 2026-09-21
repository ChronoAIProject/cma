//! Contract between verified ingress and provider turn execution.
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::Activity,
    ids::{ConversationId, IdempotencyKey},
    problem::{Problem, ProblemCode},
    seal::EventRef,
};
use futures_util::future::BoxFuture;
use std::{sync::Arc, time::Instant};

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
}
#[derive(Debug)]
pub enum LaunchOutcome {
    Accepted { response_id: String },
    Deferred,
    Refused(Problem),
}
pub trait TurnLauncher: Send + Sync {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome>;
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
