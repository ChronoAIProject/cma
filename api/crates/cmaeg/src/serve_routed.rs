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
    pub replica_routes: axum::Router,
    pub provider_start: Option<crate::telemetry::provider_start::Measurement>,
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

pub(crate) async fn start_metrics(
    address: std::net::SocketAddr,
    metrics: metrics_exporter_prometheus::PrometheusHandle,
    composition: Option<&Composition>,
) -> Result<crate::http::metrics::MetricsServer, std::io::Error> {
    let measurement = composition.and_then(|routed| routed.provider_start.clone());
    crate::http::metrics::MetricsServer::start_with_provider_start(address, metrics, measurement)
        .await
}

pub(crate) struct Deps {
    pub media: Option<Arc<MediaService>>,
    pub tool_providers: Arc<MediaToolProviders>,
    pub delivery: Arc<dyn DeliverySink>,
    pub tracker: Arc<TurnTracker>,
    pub custom: CustomDelivery,
    pub partitions: Option<crate::replicas::partition::Partitions>,
}

pub(crate) async fn compose(
    config: &Config,
    state: &mut ManagementState,
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
    let mut plans = PlanStore::new((*state.records).clone(), state.master_keys.clone());
    if config.routing.provider_start_measurement_enabled {
        use crate::telemetry::provider_start::{Measurement, MAX_SAMPLES};
        plans = plans.with_start_measurement(Measurement::new(MAX_SAMPLES)?);
    }
    let mut journal = Journal::new((*state.records).clone(), state.master_keys.clone())
        .with_activity(state.activity.clone())
        .with_execution_verifiers(state.providers.execution_verifiers.clone());
    journal.accepted_answer_receipts =
        config.routing.accepted_answer_receipts_enabled && deps.partitions.is_some();
    if let Some(partitions) = &deps.partitions {
        plans = plans.with_admission_queue(Arc::new(partitions.clone()));
        journal = journal.with_partitions(partitions.clone());
    }
    let recovery: Arc<dyn crate::turns::routed::scheduler::RecoverySource> = match &deps.partitions
    {
        Some(partitions) => Arc::new(crate::replicas::partition_discovery::Frontier {
            partitions: partitions.clone(),
            plans: plans.clone(),
        }),
        None => Arc::new(plans.clone()),
    };
    let authority: Arc<dyn Authority> = Arc::new(state.resolver.clone());
    let media_ready = state.records.media_continuations_ready().await;
    if config.routing.media_continuations_enabled && (!media_ready || deps.media.is_none()) {
        return Err(
            "media continuations require the optional Mongo index and enabled media service".into(),
        );
    }
    // This concrete continuation adapter is implemented only by CMA Responses v2.
    state.custom_existing_agent_media_providers =
        if config.routing.media_continuations_enabled && media_ready && deps.media.is_some() {
            std::collections::BTreeSet::from(["cma_codex".to_owned()])
        } else {
            Default::default()
        };
    let stream_monitor = deps
        .partitions
        .as_ref()
        .map(|_| crate::turns::routed::StreamMonitor::new(journal.clone(), deps.custom.clone()));
    let coordinator = DeliveryCoordinator::new(deps.delivery).with_custom_stream(deps.custom);
    let mut media_effects: Option<Arc<dyn crate::replicas::effects::EffectExecutor>> = None;
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
        media_effects = Some(executor.clone());
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
    let delivery = RoutedDelivery::new(plans.clone(), journal.clone(), authority, coordinator);
    let delivery = if deps.partitions.is_some() {
        delivery
            .with_stream_relay()
            .map_err(|_| "replica stream relay initialization failed")?
    } else {
        delivery
    };
    let tools = delivery.tool_replies();
    let tool_replies = Arc::new(if deps.partitions.is_some() {
        tools
            .with_replica_relay()
            .map_err(|_| "replica tool relay initialization failed")?
    } else {
        tools
    });
    let executor: Arc<dyn crate::turns::routed::scheduler::TargetExecutor> = Arc::new(
        RoutedTargetExecutor::new(port, delivery, deps.tracker, &config.delivery),
    );
    let (recovery, executor, replica_routes) = if deps.partitions.is_some() {
        let effects = match media_effects {
            Some(media) => tool_replies.replica_effect_executor_with_media(media),
            None => tool_replies.replica_effect_executor(),
        };
        let executor = journal
            .effect_executor(plans.clone(), executor, effects.clone())
            .map_err(|_| "replica effect executor initialization failed")?;
        let recovery: Arc<dyn crate::turns::routed::scheduler::RecoverySource> = Arc::new(
            crate::replicas::effect_recovery::Recovery::new(recovery, effects),
        );
        let operator = crate::replicas::unresolved::Operator::new(
            deps.partitions.clone().unwrap(),
            plans.clone(),
            journal.clone(),
        );
        let routes = tool_replies.clone().replica_relay_router().merge(
            crate::replicas::operator_http::router(
                operator.clone(),
                state.master_keys.clone(),
                &config.management.public_url,
                config.listen.port(),
            ),
        );
        let recovery: Arc<dyn crate::turns::routed::scheduler::RecoverySource> = Arc::new(
            crate::replicas::unresolved_observer::Observer::new(recovery, operator),
        );
        (recovery, executor, routes)
    } else {
        (recovery, executor, axum::Router::new())
    };
    let (scheduler, schedule) = Scheduler::new(
        recovery,
        executor,
        ScheduleConfig {
            drain_timeout: config.turns.drain,
            sweep_delay: if deps.partitions.is_some() {
                std::time::Duration::from_millis(100)
            } else {
                ScheduleConfig::default().sweep_delay
            },
            ..Default::default()
        },
    )?;
    let replay_providers = config
        .providers
        .iter()
        .filter(|provider| provider.paths.recovery.is_some())
        .map(|provider| provider.slug.as_str().to_owned())
        .collect();
    let provider_start = plans.start_measurement.clone();
    let ingress = Arc::new(
        RoutedIngress::new(
            plans,
            schedule,
            state.master_keys.clone(),
            config.routing.execution_records.clone(),
            config.routing.non_human_providers.clone(),
            replay_providers,
        )
        .with_profile_resolver(state.resolver.clone(), config.profile_revisions_enabled)
        .with_native_discovery(state.native_discord.clone())
        .with_binding_preparation(
            config.routing.binding_preparation_enabled,
            config.routing.binding_replacement_bindings.clone(),
        )
        .with_admissions_enabled(config.routing.admission_enabled)
        .with_quoted_context(config.routing.quoted_context_enabled)
        .with_sync_delivery(true)
        .with_stream_monitor(stream_monitor)
        .with_custom_media_providers(state.custom_existing_agent_media_providers.clone()),
    );
    Ok(Some(Composition {
        ingress,
        tool_replies,
        scheduler,
        media_scheduler,
        replica_routes,
        provider_start,
    }))
}
