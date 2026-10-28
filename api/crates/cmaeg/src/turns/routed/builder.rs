//! Production preparation for one already admitted routed target. Never starts
//! a response; only the provider's idempotent conversation PUT may create state.
use super::{
    dispatcher::validate_current, DefaultDestination, ExecutionError, FrozenDelivery, FrozenTurn,
    FrozenTurnBuilder,
};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::{
        launch::LaunchRequest,
        routing_store::{AdmittedPlan, StoredTarget},
    },
    media::{prepare::PrepareConfig, service::MediaService},
    providers::{ProviderClient, ProviderError},
    turns::{
        preparation,
        render_media::{self, Audience},
    },
    MediaToolProviders,
};
use cmaeg_core::{channel::ReplyChoice, seal::MasterKeyRing};
use futures_util::future::BoxFuture;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Fits within the execution journal's 30-second lease. The dispatcher still
/// rechecks/guards ownership after preparation; a timeout never submits a turn.
const MAX_PREPARATION: Duration = Duration::from_secs(20);

pub struct ProductionFrozenTurnBuilder {
    providers: Arc<ProviderClient>,
    keys: Arc<MasterKeyRing>,
    media: Option<Arc<MediaService>>,
    prepare: PrepareConfig,
    media_tool_providers: Arc<MediaToolProviders>,
}
impl ProductionFrozenTurnBuilder {
    pub fn new(providers: Arc<ProviderClient>, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            providers,
            keys,
            media: None,
            prepare: PrepareConfig::default(),
            media_tool_providers: Arc::default(),
        }
    }
    pub fn with_media(mut self, media: Option<Arc<MediaService>>, prepare: PrepareConfig) -> Self {
        self.media = media;
        self.prepare = prepare;
        self
    }
    pub fn with_media_tool_providers(mut self, providers: Arc<MediaToolProviders>) -> Self {
        self.media_tool_providers = providers;
        self
    }
    async fn build(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        current: &ResolvedChannel,
        generation: Option<u64>,
    ) -> Result<FrozenTurn, ExecutionError> {
        if !plan.targets.contains(target) {
            return Err(ExecutionError::Invalid);
        }
        validate_current(current, plan, target)?;
        if !self
            .providers
            .supports_response_replay(&target.launch.provider.slug)
            .map_err(provider_error)?
        {
            return Err(ExecutionError::Unsupported);
        }
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let remaining = target
            .launch
            .event
            .expires_at
            .min(plan.retained_until)
            .saturating_sub(now);
        if remaining <= 0 {
            return Err(ExecutionError::Expired);
        }
        let received_at = Instant::now();
        let deadline = received_at + MAX_PREPARATION.min(Duration::from_secs(remaining as u64));
        let consumer = current
            .definition
            .consumer(Some(&target.launch.consumer))
            .ok_or(ExecutionError::AuthorityChanged)?;
        let credential = consumer
            .credential
            .ok_or(ExecutionError::AuthorityChanged)?;
        let default_destination = match consumer.reply.destination {
            ReplyChoice::Origin => None,
            ReplyChoice::Default => {
                Some(DefaultDestination::capture(current).ok_or(ExecutionError::Unsupported)?)
            }
        };
        let turn = LaunchRequest {
            channel: Arc::new(current.clone()),
            activity: plan.activity.clone(),
            conversation_id: target.launch.event.conversation_id.clone(),
            event_ref: FrozenTurn::event_ref(plan, target, &self.keys)?,
            idempotency_key: target.launch.idempotency_key.clone(),
            received_at,
            trace: tracing::Span::current(),
            timing: None,
            consumer: target.launch.event.consumer.clone(),
        };
        let audience = Audience::of(&consumer, &self.media_tool_providers);
        let nonce = preparation::has_attachments(&turn.activity)
            .then(render_media::nonce)
            .flatten();
        let prepare_deadline = received_at + self.prepare.budget.min(MAX_PREPARATION);
        tokio::time::timeout_at(deadline.into(), async {
            // PUT is the existing durable, idempotent get-or-create. Never use
            // a Responses POST as a probe or create a detached preparation task.
            self.providers
                .ensure_conversation_at_generation(
                    &target.launch.provider.slug,
                    &target.launch.provider.binding_id,
                    &turn.conversation_id,
                    credential,
                    generation,
                )
                .await
                .map_err(provider_error)?;
            let media = preparation::prepare_or_empty(
                self.media.as_deref(),
                &self.prepare,
                &turn,
                audience,
                nonce,
                prepare_deadline.min(deadline),
            )
            .await;
            let rendered = preparation::render(
                &turn,
                &media.media,
                audience,
                Some(&target.launch.event.partition_key),
            );
            let body = FrozenTurn {
                conversation_id: turn.conversation_id,
                binding_generation: generation,
                idempotency_key: turn.idempotency_key,
                input: rendered.input,
                metadata: rendered.metadata,
                event_context: rendered.event_context,
                delivery: FrozenDelivery {
                    consumer: target.launch.consumer.clone(),
                    label: consumer.label.to_owned(),
                    auto_final: consumer.reply.auto_final,
                    destination: consumer.reply.destination,
                    default_destination,
                    policy: current.definition.reply.clone(),
                },
            };
            body.validate(
                plan,
                target,
                &self.keys,
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )?;
            Ok(body)
        })
        .await
        .unwrap_or(Err(ExecutionError::Unavailable))
    }
}
impl FrozenTurnBuilder for ProductionFrozenTurnBuilder {
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        Box::pin(self.build(plan, target, current, None))
    }
    fn prepare_at_generation<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
        generation: Option<u64>,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        Box::pin(self.build(plan, target, current, generation))
    }
}
pub(super) fn provider_error(error: ProviderError) -> ExecutionError {
    match error {
        ProviderError::Busy { .. } | ProviderError::Capacity => ExecutionError::Busy,
        ProviderError::InvalidRequest(_) => ExecutionError::Invalid,
        ProviderError::Rejected {
            status: 401 | 403, ..
        } => ExecutionError::AuthorityChanged,
        _ => ExecutionError::Unavailable,
    }
}
