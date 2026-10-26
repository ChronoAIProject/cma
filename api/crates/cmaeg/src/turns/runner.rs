//! Acceptance budget is independent of the detached, bounded provider consumer.
use super::{
    consume,
    outcomes::TurnOutcome,
    preparation,
    preparation::has_attachments,
    render_media::{self, Audience, RenderMedia},
    sequencer::{Sequencer, Ticket},
    tracker::TurnTracker,
};
use crate::{
    delivery::router::DeliverySink,
    ingress::launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
    media::{prepare::PrepareConfig, service::MediaService},
    providers::{ProviderClient, ProviderError, TurnRequest, TurnStream},
    telemetry::timing::MediaLabel,
    MediaToolProviders, TurnConfig,
};
use cmaeg_core::{
    ids::ConsumerId,
    problem::{Problem, ProblemCode},
};
use futures_util::future::BoxFuture;
use ring::rand::{SecureRandom, SystemRandom};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, OwnedSemaphorePermit};

/// Headroom over the predecessor's own bound, so a fence normally ends with
/// its predecessor's start rather than with its own timeout.
const FENCE_SLACK: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct TurnRunner {
    providers: Arc<ProviderClient>,
    tracker: Arc<TurnTracker>,
    delivery: Arc<dyn DeliverySink>,
    config: TurnConfig,
    /// Providers whose profile bindings install the gateway MCP tools; only
    /// their agents are told how to open an attachment through a tool.
    media_tool_providers: Arc<MediaToolProviders>,
    /// Best-effort provider start order within a conversation while a media
    /// turn is part of it.
    sequencer: Arc<Sequencer>,
    /// The media service while inbound media is enabled; `None` renders
    /// attachment descriptors only.
    media: Option<Arc<MediaService>>,
    prepare: PrepareConfig,
    activity: Arc<dyn super::observer::ActivityObserver>,
}
impl TurnRunner {
    pub fn new(
        providers: Arc<ProviderClient>,
        tracker: Arc<TurnTracker>,
        delivery: Arc<dyn DeliverySink>,
        config: TurnConfig,
    ) -> Self {
        Self {
            providers,
            tracker,
            delivery,
            config,
            media_tool_providers: Arc::default(),
            sequencer: Arc::default(),
            media: None,
            prepare: PrepareConfig::default(),
            activity: Arc::new(super::observer::NoopObserver),
        }
    }
    /// Observe durable dispatch facts through the composition's shared observer.
    pub fn with_activity(mut self, activity: Arc<dyn super::observer::ActivityObserver>) -> Self {
        self.activity = activity;
        self
    }
    /// Prepare media turns with `media` under `prepare`'s bounds.
    pub fn with_media(mut self, media: Option<Arc<MediaService>>, prepare: PrepareConfig) -> Self {
        self.media = media;
        self.prepare = prepare;
        self
    }
    /// The order chains this runner keeps, for tests and diagnostics.
    pub fn sequencer(&self) -> &Sequencer {
        &self.sequencer
    }
    pub fn with_media_tool_providers(mut self, providers: Arc<MediaToolProviders>) -> Self {
        self.media_tool_providers = providers;
        self
    }
    async fn execute(
        &self,
        turn: Arc<LaunchRequest>,
        sender: oneshot::Sender<LaunchOutcome>,
        mut ticket: Ticket,
    ) {
        use crate::telemetry::{
            names,
            operations::{provider_error, Operation},
        };
        let consumer = match turn
            .channel
            .consumer(turn.consumer.as_ref())
            .ok_or_else(unconfigured)
        {
            Ok(consumer) => consumer,
            Err(error) => return self.unserved(turn, sender, error),
        };
        let provider = consumer.provider.slug.to_string();
        let mut observation = Operation::new(names::TURN_DURATION, names::PROVIDER, &provider);
        let deadline = turn.received_at + self.config.turn_timeout;
        let audience = Audience::of(&consumer, &self.media_tool_providers);
        // One nonce per turn: busy retries resend this exact input under the
        // same idempotency key. Text-only turns make no RNG call.
        let nonce = if has_attachments(&turn.activity) {
            render_media::nonce()
        } else {
            None
        };
        let prepare_deadline = (turn.received_at + self.prepare.budget).min(deadline);
        let mut admitted = Some(sender);
        let result = tokio::time::timeout_at(deadline.into(), async {
            let fenced = ticket.waits();
            // A predecessor may itself prepare for the whole budget before its
            // busy retries, so the fence covers both.
            let fence = self.config.busy_retry_max + self.prepare.budget + FENCE_SLACK;
            let (_, prepared) = tokio::join!(
                ticket.wait(fence),
                preparation::prepare_or_empty(
                    self.media.as_deref(),
                    &self.prepare,
                    &turn,
                    audience,
                    nonce,
                    prepare_deadline
                ),
            );
            if let Some(timing) = &turn.timing {
                if fenced {
                    timing.mark_media(MediaLabel::Fenced);
                } else if prepared.eager {
                    timing.mark_media(MediaLabel::Prepared);
                }
            }
            let started = self.start(&turn, &prepared.media, audience).await;
            // The successor may start once this start has returned, accepted
            // or not; a later failure is this turn's own.
            ticket.release();
            let stream = started.map_err(|error| match error {
                ProviderError::Capacity | ProviderError::Busy { .. } => {
                    (ProblemCode::ProviderBusy, "provider_busy")
                }
                _ => (ProblemCode::ProviderUnavailable, "provider_unavailable"),
            })?;
            consume::consume(
                stream,
                turn.clone(),
                &provider,
                &*self.delivery,
                &mut admitted,
            )
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
        self.delivery.terminal(
            turn,
            TurnOutcome::Failed {
                code: error.1.into(),
                message: failure_message(error.1).into(),
            },
        );
    }
    /// Fail a turn whose consumer its channel does not configure, as any
    /// other provider refusal fails, before any provider call.
    fn unserved(
        &self,
        turn: Arc<LaunchRequest>,
        sender: oneshot::Sender<LaunchOutcome>,
        error: ProviderError,
    ) {
        tracing::error!(
            consumer = turn.consumer.as_ref().map(ConsumerId::as_str),
            %error,
            "turn names a consumer its channel does not configure"
        );
        let code = "provider_unavailable";
        consume::refuse(&mut Some(sender), ProblemCode::ProviderUnavailable, code);
        self.delivery.terminal(
            turn,
            TurnOutcome::Failed {
                code: code.into(),
                message: failure_message(code).into(),
            },
        );
    }
    async fn start(
        &self,
        turn: &LaunchRequest,
        media: &RenderMedia,
        audience: Audience,
    ) -> Result<TurnStream, ProviderError> {
        let consumer = turn
            .channel
            .consumer(turn.consumer.as_ref())
            .ok_or_else(unconfigured)?;
        let credential = consumer.credential.ok_or_else(|| {
            ProviderError::InvalidRequest("channel has no provider credential".into())
        })?;
        let rendered = preparation::render(turn, media, audience, None);
        let request = TurnRequest {
            timing: turn.timing.clone(),
            conversation_id: turn.conversation_id.clone(),
            input: rendered.input,
            idempotency_key: turn.idempotency_key.clone(),
            metadata: rendered.metadata,
            agent_key: credential.clone(),
            event_context: Some(rendered.event_context),
        };
        let retry_deadline = Instant::now() + self.config.busy_retry_max;
        let mut ensured = false;
        let mut change = crate::activity::Change::state(
            consumer.id.as_str(),
            crate::activity::ConsumerState::Dispatching,
            None,
        );
        change.provider = Some(consumer.provider.slug.to_string());
        change.display_name = Some(consumer.label.to_owned());
        change.conversation_id = Some(turn.conversation_id.to_string());
        self.activity
            .checkpoint(
                &turn.channel.definition.record_id,
                &turn.activity.event_id,
                change,
            )
            .await;
        loop {
            match self
                .providers
                .start_turn_immediate(&consumer.provider.slug, request.clone())
                .await
            {
                Err(ProviderError::ConversationMissing) if !ensured => {
                    ensured = true;
                    self.providers
                        .ensure_conversation(
                            &consumer.provider.slug,
                            &consumer.provider.binding_id,
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
impl TurnRunner {
    /// Spawn the turn and wait for its acceptance. `hold` is the shared
    /// ingress permit, when the caller passed one.
    fn launch_inner(
        &self,
        mut request: LaunchRequest,
        hold: Option<OwnedSemaphorePermit>,
    ) -> BoxFuture<'_, LaunchOutcome> {
        // Before any await, so the chain follows the order of launch calls.
        let media = has_attachments(&request.activity);
        let ticket = self.sequencer.enter(
            (request.channel.id.clone(), request.conversation_id.clone()),
            media,
        );
        let fenced = ticket.waits();
        Box::pin(async move {
            let deadline = request.received_at + self.config.accept_budget;
            request.trace = tracing::Span::current();
            let turn = Arc::new(request);
            let (sender, accepted) = oneshot::channel();
            let runner = self.clone();
            let pending = turn.clone();
            if let Err(reason) = self.tracker.spawn(&turn, async move {
                runner.execute(pending, sender, ticket).await;
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
            if media || fenced {
                // Preparation and the order fence may take up to the
                // acceptance budget, and the shared permit must not be held
                // through them. The acceptance wait below keeps every
                // transport's semantics. A released handler holds only a
                // response context of under 4 KiB, because bodies were
                // released before launch, and the turn tracker bounds such
                // handlers at 4 096.
                drop(hold);
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
impl TurnLauncher for TurnRunner {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        self.launch_inner(request, None)
    }
    fn launch_holding(
        &self,
        request: LaunchRequest,
        hold: OwnedSemaphorePermit,
    ) -> BoxFuture<'_, LaunchOutcome> {
        self.launch_inner(request, Some(hold))
    }
}
/// The serving consumer is gone from the channel's definition.
fn unconfigured() -> ProviderError {
    ProviderError::InvalidRequest("consumer is not configured".into())
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
