//! Service composition: build every component from configuration, then serve
//! HTTP until a termination signal drains turns, deliveries and connections.

use std::{future::IntoFuture, net::SocketAddr, time::Instant};

use tokio::sync::oneshot;
use tower_http::{catch_panic::CatchPanicLayer, limit::RequestBodyLimitLayer};

use crate::{
    auth, connections, console, delivery, http, ingress, interactions, mcp, media, providers,
    records, router_with_state, service_auth, telegram, telemetry, tools, turns, Config,
    REQUEST_BODY_LIMIT_BYTES,
};

/// Compose CMAEG from `config` and run it until a process termination signal arrives.
pub(crate) async fn serve(config: Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service_auth = service_auth::ServiceAuth::from_config(&config)?;
    // Prove the renewable credential before advertising gateway readiness.
    service_auth.bearer().await?;
    let providers = std::sync::Arc::new(
        providers::ProviderClient::from_config(&config)?.with_service_auth(service_auth.clone()),
    );
    let records = std::sync::Arc::new(record_client(&config, service_auth)?);
    let creator_auth = std::sync::Arc::new(auth::CreatorAuthenticator::new(
        &config.management.nyxid_base_url,
    )?);
    // No-op observers and no queue while CMAEG_CONSOLE_ENABLED is off.
    let mut observers = console::observer::Observers::new(config.console.enabled);
    let mut state = http::ManagementState::observed(
        providers.clone(),
        records,
        creator_auth,
        config.management.clone(),
        config.channel_cache.clone(),
        observers.index.clone(),
    );
    let telegram = std::sync::Arc::new(telegram::TelegramClient::production(
        config.delivery.reply_timeout,
    )?);
    state.telegram = Some(telegram.clone());
    state.media_enabled = config.media.enabled;
    let _reachability_probe = state.readiness.start_probe(state.records.clone());
    let tracker = turns::TurnTracker::new(state.readiness.clone());
    let (inbound, outcomes) = tokio::sync::mpsc::channel(256);
    let delivery: std::sync::Arc<dyn delivery::router::DeliverySink> =
        std::sync::Arc::new(delivery::router::ShardedScheduler::new(inbound));
    let waiters = std::sync::Arc::new(delivery::sync::SyncRegistry::default());
    let interactions = std::sync::Arc::new(interactions::InteractionStore::new(
        (*state.records).clone(),
        state.master_keys.clone(),
        providers.clone(),
    ));
    let delivery_engine = delivery::engine::DeliveryEngine::start_observed(
        outcomes,
        state.master_keys.clone(),
        delivery::relay_client::RelayClient::new(
            &config.management.nyxid_base_url,
            config.delivery.reply_timeout,
        )?
        .with_interactions(interactions.clone()),
        config.delivery.clone(),
        tracker.clone(),
        delivery::custom::CustomDelivery::new(
            config.management.public_url.clone(),
            waiters.clone(),
        ),
        observers.activity.clone(),
    );
    let tool_providers = std::sync::Arc::new(config.media.tool_providers.clone());
    // Installed before the media transformer's setup so its readiness gauge
    // is recorded; the metrics listener still binds after the gateway's.
    let metrics = telemetry::install()?;
    // After the recorder, which registers its gauge. `None` while
    // CMAEG_CONSOLE_ENABLED is off: then no /console route exists.
    let console = console::compose(&config, &state, observers.queue.take())?;
    // Before the listener binds: a configured transformer must pass its
    // sandbox self-check, or the gateway does not start.
    let transformer = media::transform::setup::start_configured(&config).await?;
    // The one media service of the process, or `None` while
    // CMAEG_MEDIA_ENABLED=false so nothing is allocated. Turn preparation and
    // the attachment tools share it, so they share its budgets, cache and
    // transformer.
    let media = media::service::compose(
        &config,
        media::service::ComposeDeps {
            keys: state.master_keys.clone(),
            tool_providers: tool_providers.clone(),
            telegram: telegram.clone(),
            transformer,
        },
    )?;
    let runner = std::sync::Arc::new(
        turns::TurnRunner::new(
            providers.clone(),
            tracker.clone(),
            delivery.clone(),
            config.turns.clone(),
        )
        .with_media_tool_providers(tool_providers)
        .with_media(
            media.clone(),
            media::prepare::PrepareConfig {
                budget: config.media.prepare_budget,
                eager_audio_max_seconds: config.stt.eager_max_seconds,
            },
        ),
    );
    let ingress = ingress::IngressState::new(
        state.resolver.clone(),
        state.master_keys.clone(),
        config.ingress,
        observers.launcher(runner.clone()),
    )?
    .with_telegram(telegram)
    .with_interactions(interactions)
    .with_media_enabled(config.media.enabled)
    .with_activity_observer(observers.activity.clone());
    let tools_state = tools::ToolsState {
        resolver: state.resolver.clone(),
        keys: state.master_keys.clone(),
        providers,
        delivery,
        tracker: tracker.clone(),
        public_origin: config.management.public_url.clone(),
        media,
        downloads: std::sync::Arc::new(
            tools::MediaDownloads::default().with_link_ttl(config.media.link_ttl_seconds),
        ),
    };
    let connections = connections::ConnectionService::new(state.clone(), tools_state.clone());
    let ingress = ingress.with_receipt_observer(std::sync::Arc::new(connections.clone()));
    let _connection_worker = connections.start_completion_worker(runner);
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let _metrics = http::metrics::MetricsServer::start(config.metrics_listen, metrics).await?;
    tracing::info!(bind = %config.listen, "CMAEG listening");

    let (shutdown_started, shutdown_signal) = oneshot::channel();
    let server = axum::serve(
        listener,
        router_with_state(state)
            .merge(http::events::router(http::events::CustomIngressState {
                ingress: ingress.clone(),
                waiters,
            }))
            .merge(http::connections::router(connections.clone()))
            .merge(http::connection_browser::router(connections))
            .merge(http::telegram::router(ingress.clone()))
            .merge(http::callbacks::router(ingress))
            .merge(http::tools::router(tools_state.clone()))
            .merge(http::media::router(tools_state.clone()))
            .merge(mcp::router(tools_state))
            .merge(console.unwrap_or_default())
            .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES))
            .layer(CatchPanicLayer::custom(http::problems::panic_response))
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        wait_for_shutdown_signal().await;
        tracker.start_draining();
        let deadline = Instant::now() + config.turns.drain;
        if !tracker.drain(deadline).await {
            tracing::warn!("turn drain deadline reached; remaining consumers cancelled");
        }
        if !delivery_engine.drain(deadline).await {
            tracing::warn!(
                "delivery drain deadline reached; remaining sends cancelled with unknown outcome"
            );
        }
        let _ = shutdown_started.send(deadline);
    })
    .into_future();
    tokio::pin!(server);

    tokio::select! {
        result = &mut server => result?,
        deadline = shutdown_signal => {
            let deadline = deadline?;
            match tokio::time::timeout_at(deadline.into(), &mut server).await {
                Ok(result) => result?,
                Err(_) => tracing::warn!("HTTP drain deadline reached"),
            }
        }
    }
    Ok(())
}

/// The CMA channel-record client, authenticated as the gateway service.
pub(crate) fn record_client(
    config: &Config,
    service_auth: service_auth::ServiceAuth,
) -> Result<records::RecordClient, records::RecordError> {
    Ok(records::RecordClient::new(
        &config.management.cma_base_url,
        config.management.cma_service_token.as_str(),
    )?
    .with_service_auth(service_auth))
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        match terminate {
            Ok(mut signal) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if let Err(error) = result { tracing::warn!(%error, "ctrl-c signal listener stopped"); }
                    }
                    _ = signal.recv() => {}
                }
            }
            Err(error) => {
                tracing::warn!(%error, "SIGTERM listener unavailable; waiting for ctrl-c");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
