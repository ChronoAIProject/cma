//! Acceptance budget is independent of the detached, bounded provider consumer.
use super::{
    consume, metadata,
    outcomes::{DeliveryCommand, TurnOutcome},
    render_input,
    tracker::TurnTracker,
};
use crate::{
    ingress::launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
    providers::{ProviderClient, ProviderError, TurnRequest, TurnStream},
    TurnConfig,
};
use cmaeg_core::problem::{Problem, ProblemCode};
use futures_util::future::BoxFuture;
use ring::rand::{SecureRandom, SystemRandom};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct TurnRunner {
    providers: Arc<ProviderClient>,
    tracker: Arc<TurnTracker>,
    delivery: mpsc::Sender<DeliveryCommand>,
    config: TurnConfig,
}
impl TurnRunner {
    pub fn new(
        providers: Arc<ProviderClient>,
        tracker: Arc<TurnTracker>,
        delivery: mpsc::Sender<DeliveryCommand>,
        config: TurnConfig,
    ) -> Self {
        Self {
            providers,
            tracker,
            delivery,
            config,
        }
    }
    async fn execute(&self, turn: Arc<LaunchRequest>, sender: oneshot::Sender<LaunchOutcome>) {
        use crate::telemetry::{
            names,
            operations::{provider_error, Operation},
        };
        let provider = turn.channel.definition.provider.slug.to_string();
        let mut observation = Operation::new(names::TURN_DURATION, names::PROVIDER, &provider);
        let deadline = turn.received_at + self.config.turn_timeout;
        let mut admitted = Some(sender);
        let result = tokio::time::timeout_at(deadline.into(), async {
            let stream = self.start(&turn).await.map_err(|error| match error {
                ProviderError::Capacity | ProviderError::Busy { .. } => {
                    (ProblemCode::ProviderBusy, "provider_busy")
                }
                _ => (ProblemCode::ProviderUnavailable, "provider_unavailable"),
            })?;
            consume::consume(stream, turn.clone(), &self.delivery, &mut admitted)
                .await
                .map_err(|reason| (ProblemCode::ProviderUnavailable, reason))
        })
        .await;
        let error = match result {
            Ok(Ok(outcome)) => {
                observation.finish(outcome);
                if outcome != "completed" {
                    provider_error(&provider, outcome);
                }
                return;
            }
            Ok(Err(error)) => error,
            Err(_) => (ProblemCode::ProviderUnavailable, "turn_timeout"),
        };
        observation.finish(error.1);
        provider_error(&provider, error.1);
        consume::refuse(&mut admitted, error.0, error.1);
        // A stalled delivery sink must not keep a timed-out turn alive indefinitely.
        let command = DeliveryCommand::StreamOutcome {
            turn,
            outcome: TurnOutcome::Failed {
                code: error.1.into(),
                message: failure_message(error.1).into(),
            },
        };
        if self.delivery.try_send(command).is_err() {
            tracing::warn!(
                reason = error.1,
                "turn failure could not reach delivery sink"
            );
        }
    }
    async fn start(&self, turn: &LaunchRequest) -> Result<TurnStream, ProviderError> {
        let definition = &turn.channel.definition;
        let credential = definition
            .secrets
            .provider_credential
            .as_ref()
            .ok_or_else(|| {
                ProviderError::InvalidRequest("channel has no provider credential".into())
            })?;
        let request = TurnRequest {
            timing: turn.timing.clone(),
            conversation_id: turn.conversation_id.clone(),
            input: render_input::render(&turn.activity, definition),
            idempotency_key: turn.idempotency_key.clone(),
            metadata: metadata::render(turn),
            agent_key: credential.clone(),
            event_context: Some(crate::providers::EventContextSnapshot {
                schema_version: 1,
                channel_id: turn.channel.id.clone(),
                conversation_id: turn.conversation_id.clone(),
                activity: turn.activity.clone(),
                event_ref: turn.event_ref.clone(),
            }),
        };
        let retry_deadline = Instant::now() + self.config.busy_retry_max;
        let mut ensured = false;
        loop {
            match self
                .providers
                .start_turn_immediate(&definition.provider.slug, request.clone())
                .await
            {
                Err(ProviderError::ConversationMissing) if !ensured => {
                    ensured = true;
                    self.providers
                        .ensure_conversation(
                            &definition.provider.slug,
                            &definition.provider.binding_id,
                            &turn.conversation_id,
                            credential,
                        )
                        .await?;
                }
                Err(ProviderError::Busy { retry_after }) => {
                    let remaining = retry_deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(ProviderError::Busy { retry_after });
                    }
                    let minimum = retry_after
                        .unwrap_or(Duration::from_millis(500))
                        .max(Duration::from_millis(1));
                    // Retry-After is a minimum, including while a cold conversation
                    // is preparing. Add jitter without probing before that time.
                    let delay = minimum
                        .saturating_add(jitter(minimum.min(Duration::from_millis(500))))
                        .min(remaining);
                    tokio::time::sleep(delay).await;
                    if Instant::now() >= retry_deadline {
                        return Err(ProviderError::Busy { retry_after });
                    }
                }
                result => return result,
            }
        }
    }
}
impl TurnLauncher for TurnRunner {
    fn launch(&self, mut request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        Box::pin(async move {
            let deadline = request.received_at + self.config.accept_budget;
            request.trace = tracing::Span::current();
            let turn = Arc::new(request);
            let (sender, accepted) = oneshot::channel();
            let runner = self.clone();
            let pending = turn.clone();
            if let Err(reason) = self.tracker.spawn(&turn, async move {
                runner.execute(pending, sender).await;
            }) {
                return LaunchOutcome::Refused(Problem::new(
                    if reason == "draining" {
                        ProblemCode::Draining
                    } else {
                        ProblemCode::ProviderBusy
                    },
                    503,
                    reason,
                ));
            }
            match tokio::time::timeout_at(deadline.into(), accepted).await {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(_)) => LaunchOutcome::Refused(Problem::new(
                    ProblemCode::ProviderUnavailable,
                    503,
                    "turn stopped before acceptance",
                )),
                Err(_) => LaunchOutcome::Deferred,
            }
        })
    }
}
fn jitter(max: Duration) -> Duration {
    let mut bytes = [0; 8];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        return max;
    }
    let ratio = u64::from_le_bytes(bytes) as f64 / u64::MAX as f64;
    max.mul_f64(ratio).max(Duration::from_micros(100))
}
fn failure_message(code: &str) -> &'static str {
    match code {
        "turn_timeout" => "provider turn exceeded its deadline",
        "stream_lost" => "provider stream ended without a terminal event",
        "provider_busy" => "provider concurrency is exhausted",
        "output_too_large" => "provider output exceeded the gateway limit",
        _ => "provider turn could not be completed",
    }
}
