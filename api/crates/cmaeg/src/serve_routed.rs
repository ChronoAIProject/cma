//! Opt-in service composition for durable routed admission and execution.
mod lifecycle;
pub(crate) use lifecycle::Worker;

use crate::{
    delivery::{custom::CustomDelivery, durable::DeliveryCoordinator, router::DeliverySink},
    http::ManagementState,
    ingress::{routed::RoutedIngress, routing_store::PlanStore},
    media::{prepare::PrepareConfig, service::MediaService},
    turns::{
        routed::{
            scheduler::{ScheduleConfig, Scheduler},
            Authority, HttpExecutionPort, Journal, MediaDelivery, MediaObligationStore,
            MediaWorkIndex, MediaWorker, ProductionFrozenTurnBuilder, RoutedDelivery,
            RoutedTargetExecutor, RoutedToolReplies,
        },
        TurnTracker,
    },
    Config, MediaToolProviders,
};
use std::sync::Arc;

pub(crate) struct Composition {
    pub ingress: Arc<RoutedIngress>,
    pub tool_replies: Arc<RoutedToolReplies>,
    pub scheduler: Scheduler,
    pub media_scheduler: Option<Scheduler>,
}
pub(crate) fn start_workers(
    composition: Option<Composition>,
    readiness: crate::http::readiness::Readiness,
) -> Vec<Worker> {
    let Some(composition) = composition else {
        return Vec::new();
    };
    let mut workers = vec![Worker::start(composition.scheduler, readiness.clone())];
    if let Some(scheduler) = composition.media_scheduler {
        workers.push(Worker::start(scheduler, readiness));
    }
    workers
}

pub(crate) struct Deps {
    pub media: Option<Arc<MediaService>>,
    pub tool_providers: Arc<MediaToolProviders>,
    pub delivery: Arc<dyn DeliverySink>,
    pub tracker: Arc<TurnTracker>,
    pub custom: CustomDelivery,
}

pub(crate) async fn compose(
    config: &Config,
    state: &ManagementState,
    deps: Deps,
) -> Result<Option<Composition>, Box<dyn std::error::Error + Send + Sync>> {
    config.routing.validate()?;
    if !config.routing.recovery_enabled {
        return Ok(None);
    }
    if config.routing.binding_preparation_enabled
        && !state
            .providers
            .supports_binding_preparation(&cmaeg_core::ids::ProviderSlug::new("cma_codex")?)
    {
        return Err(
            "binding preparation requires exact configured CMA management authority".into(),
        );
    }
    let plans = PlanStore::new((*state.records).clone(), state.master_keys.clone());
    let journal = Journal::new((*state.records).clone(), state.master_keys.clone())
        .with_activity(state.activity.clone());
    let authority: Arc<dyn Authority> = Arc::new(state.resolver.clone());
    let media_ready = state.records.media_continuations_ready().await;
    if config.routing.media_continuations_enabled && (!media_ready || deps.media.is_none()) {
        return Err(
            "media continuations require the optional Mongo index and enabled media service".into(),
        );
    }
    let coordinator = DeliveryCoordinator::new(deps.delivery).with_custom_stream(deps.custom);
    let media_scheduler = if media_ready {
        let media = deps
            .media
            .clone()
            .ok_or("existing media continuation reader requires enabled media service")?;
        let index = MediaWorkIndex::new(journal.clone(), plans.clone());
        let store = MediaObligationStore(journal.clone());
        let media_delivery = Arc::new(MediaDelivery {
            store: store.clone(),
            authority: authority.clone(),
            providers: state.providers.clone(),
            coordinator: coordinator.clone(),
        });
        let executor = Arc::new(MediaWorker {
            index: index.clone(),
            plans: plans.clone(),
            store,
            authority: authority.clone(),
            providers: state.providers.clone(),
            media,
            delivery: media_delivery,
            worker: format!("media-{}", uuid::Uuid::new_v4()),
        });
        let (scheduler, _) = Scheduler::new(
            Arc::new(index),
            executor,
            ScheduleConfig {
                concurrency: 2,
                queue_capacity: 64,
                drain_timeout: config.turns.drain,
                ..Default::default()
            },
        )?;
        Some(scheduler)
    } else {
        None
    };
    let builder =
        ProductionFrozenTurnBuilder::new(state.providers.clone(), state.master_keys.clone())
            .with_media(
                deps.media,
                PrepareConfig {
                    budget: config.media.prepare_budget,
                    eager_audio_max_seconds: config.stt.eager_max_seconds,
                },
            )
            .with_media_tool_providers(deps.tool_providers)
            .with_media_continuations(config.routing.media_continuations_enabled);
    let port = Arc::new(
        HttpExecutionPort::new(
            state.providers.clone(),
            Arc::new(builder),
            plans.clone(),
            journal.clone(),
            authority.clone(),
        )
        .with_media_continuations(media_ready),
    );
    let delivery = RoutedDelivery::new(plans.clone(), journal, authority, coordinator);
    let tool_replies = Arc::new(delivery.tool_replies());
    let executor = Arc::new(RoutedTargetExecutor::new(
        port,
        delivery,
        deps.tracker,
        &config.delivery,
    ));
    let (scheduler, schedule) = Scheduler::new(
        Arc::new(plans.clone()),
        executor,
        ScheduleConfig {
            drain_timeout: config.turns.drain,
            ..Default::default()
        },
    )?;
    let replay_providers = config
        .providers
        .iter()
        .filter(|provider| provider.paths.recovery.is_some())
        .map(|provider| provider.slug.as_str().to_owned())
        .collect();
    let ingress = Arc::new(
        RoutedIngress::new(
            plans,
            schedule,
            state.master_keys.clone(),
            config.routing.execution_records.clone(),
            config.routing.non_human_providers.clone(),
            replay_providers,
        )
        .with_native_discovery(state.native_discord.clone())
        .with_binding_preparation(
            config.routing.binding_preparation_enabled,
            config.routing.binding_replacement_bindings.clone(),
        )
        .with_admissions_enabled(config.routing.admission_enabled)
        .with_quoted_context(config.routing.quoted_context_enabled)
        .with_sync_delivery(true),
    );
    Ok(Some(Composition {
        ingress,
        tool_replies,
        scheduler,
        media_scheduler,
    }))
}
