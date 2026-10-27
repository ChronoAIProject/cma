//! Service composition: build every component from configuration, then serve
//! HTTP until a termination signal drains turns, deliveries and connections.

use std::{future::IntoFuture, net::SocketAddr, time::Instant};

use tokio::sync::oneshot;
use tower_http::{catch_panic::CatchPanicLayer, limit::RequestBodyLimitLayer};

use crate::{
    connections, console, delivery, http, ingress, interactions, mcp, media, providers, records,
    router_with_state, service_auth, telegram, telemetry, tools, turns, Config,
    REQUEST_BODY_LIMIT_BYTES,
};

/// Compose CMAEG from `config` and run it until a process termination signal arrives.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve(
    config: Config,
    complete_replies: bool,
    native_media: bool,
    relay_text_edits: bool,
    native_discord_edits: bool,
    native_discord_effects: bool,
    native_discord_media_effects: bool,
    native_discord_threads: bool,
    lark_cards: bool,
    lark_forms: bool,
    whatsapp_templates: bool,
    whatsapp_interactions: bool,
    discord_interactions: bool,
    discord_forms: bool,
    discord_multiselect: bool,
    discord_card_edits: bool,
    discord_card_effects: bool,
    consumer_interactions: bool,
    telegram_effects: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service_auth = service_auth::ServiceAuth::from_config(&config)?;
    // Prove the renewable credential before advertising gateway readiness.
    service_auth.bearer().await?;
    let providers = std::sync::Arc::new(
        providers::ProviderClient::from_config(&config)?.with_service_auth(service_auth.clone()),
    );
    let (records, creator_auth, browser) =
        crate::serve_storage::compose(&config, service_auth).await?;
    let resource_connections = connections::relay_resources::ResourceConnections::new(
        creator_auth.clone(),
        &config.management.nyxid_base_url,
    )?;
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
    observers.activity = std::sync::Arc::new(crate::activity::ObservedActivity::new(
        state.activity.clone(),
        observers.activity.clone(),
    ));
    let telegram = std::sync::Arc::new(telegram::TelegramClient::production(
        config.delivery.reply_timeout,
    )?);
    state.telegram = Some(telegram.clone());
    state.media_enabled = config.media.enabled;
    state.native_bot_mentions_writes_enabled = config.native_bot_mentions_writes_enabled;
    state.thread_contract_writes_enabled = config.thread_contract_writes_enabled;
    state.multi_consumer_writes_enabled = config.multi_consumer_writes_enabled;
    state.dynamic_sources_writes_enabled = config.dynamic_sources_writes_enabled;
    state.relay_raw_verification_writes_enabled = config.relay_raw_verification_writes_enabled;
    state.lark_resource_writes_enabled = config.lark_resource_writes_enabled;
    state.native_whatsapp_writes_enabled = config.native_whatsapp_writes_enabled;
    state.native_whatsapp_admission_enabled =
        config.routing.admission_enabled && config.routing.recovery_enabled;
    state.lark_message_effects_writes_enabled = config.lark_message_effects_writes_enabled;
    state.native_discord_writes_enabled = config.native_discord_writes_enabled;
    state.native_telegram_message_events_writes_enabled =
        config.native_telegram_message_events_writes_enabled;
    state.native_telegram_durable_admission_enabled =
        config.routing.recovery_enabled && config.routing.admission_enabled;
    state.native_discord_thread_history_enabled = config.native_discord_thread_history_enabled;
    state.native_discord_reactions_enabled = config.native_discord_reactions_enabled;
    state.native_discord = crate::discord::selection::Selection::new(&config.native_discord);
    state.native_discord_admission_enabled = config.routing.admission_enabled;
    state.outbound_messages_writes_enabled = config.outbound_messages_writes_enabled;
    state.outbound_relay = Some(
        delivery::relay_client::RelayClient::new(
            &config.management.nyxid_base_url,
            config.delivery.reply_timeout,
        )?
        .with_native_media(native_media)
        .with_whatsapp_templates(whatsapp_templates),
    );
    let whatsapp = crate::whatsapp::Client::production()?;
    state.whatsapp = Some(std::sync::Arc::new(whatsapp.clone()));
    let whatsapp_receipts = std::sync::Arc::new(crate::whatsapp::receipts::Store::new(
        (*state.records).clone(),
        state.master_keys.clone(),
    ));
    let _reachability_probe = state.readiness.start_probe(state.records.clone());
    let tracker = turns::TurnTracker::new(state.readiness.clone());
    let (inbound, outcomes) = tokio::sync::mpsc::channel(256);
    let delivery: std::sync::Arc<dyn delivery::router::DeliverySink> =
        std::sync::Arc::new(delivery::router::ShardedScheduler::new(inbound));
    let waiters = std::sync::Arc::new(delivery::sync::SyncRegistry::default());
    let custom_delivery = delivery::custom::CustomDelivery::new(
        config.management.public_url.clone(),
        waiters.clone(),
    )
    .with_activity(observers.activity.clone());
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
        .with_whatsapp(whatsapp, whatsapp_receipts.clone())
        .with_complete_replies(complete_replies)
        .with_native_media(native_media)
        .with_text_edits(relay_text_edits)
        .with_discord_reactions(config.native_discord_reactions_enabled)
        .with_native_discord_edits(native_discord_edits)
        .with_native_discord_effects(native_discord_effects)
        .with_native_discord_media_effects(native_discord_media_effects)
        .with_lark_cards(lark_cards)
        .with_lark_forms(lark_forms)
        .with_whatsapp_templates(whatsapp_templates)
        .with_whatsapp_interactions(whatsapp_interactions)
        .with_discord_interactions(discord_interactions)
        .with_discord_forms(discord_forms)
        .with_discord_multiselect(discord_multiselect)
        .with_discord_card_edits(discord_card_edits)
        .with_consumer_interactions(consumer_interactions)
        .with_telegram_effects(telegram_effects)
        .with_interactions(interactions.clone()),
        config.delivery.clone(),
        tracker.clone(),
        custom_delivery.clone(),
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
            resolver: state.resolver.clone(),
            keys: state.master_keys.clone(),
            tool_providers: tool_providers.clone(),
            telegram: telegram.clone(),
            transformer,
        },
    )?;
    let routed = crate::serve_routed::compose(
        &config,
        &state,
        crate::serve_routed::Deps {
            media: media.clone(),
            tool_providers: tool_providers.clone(),
            delivery: delivery.clone(),
            tracker: tracker.clone(),
            custom: custom_delivery,
        },
    )?;
    let runner = std::sync::Arc::new(
        turns::TurnRunner::new(
            providers.clone(),
            tracker.clone(),
            delivery.clone(),
            config.turns.clone(),
        )
        .with_activity(observers.activity.clone())
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
        std::sync::Arc::new(turns::observer::ObservedLauncher::new(
            runner.clone(),
            observers.activity.clone(),
        )),
    )?
    .with_whatsapp_receipts(whatsapp_receipts)
    .with_telegram(telegram.clone())
    .with_telegram_effects(telegram_effects)
    .with_interactions(interactions)
    .with_media_enabled(config.media.enabled)
    .with_routed(
        routed
            .as_ref()
            .map(|composition| composition.ingress.clone()),
    )
    .with_activity_observer(observers.activity.clone());
    let tools_state = tools::ToolsState {
        discord_thread_history: config
            .native_discord_thread_history_enabled
            .then(|| crate::discord::DiscordRest::new().map(tools::DiscordThreadHistory::new))
            .transpose()?
            .map(std::sync::Arc::new),
        complete_replies_enabled: complete_replies,
        routed_replies: routed
            .as_ref()
            .map(|composition| composition.tool_replies.clone()),
        telegram_effects_issuance_enabled: telegram_effects,
        lark_effects_issuance_enabled: config.lark_message_effects_writes_enabled,
        lark_effects: Some(std::sync::Arc::new(
            tools::message_effects::lark_transport::LarkEffectsClient::new(
                &config.management.nyxid_base_url,
            )?,
        )),
        telegram_effects: Some(std::sync::Arc::new(tools::MessageEffectsClient::new(
            telegram,
        ))),
        whatsapp_templates_enabled: whatsapp_templates,
        discord_interactions_enabled: discord_interactions,
        whatsapp_interactions_enabled: whatsapp_interactions,
        consumer_interactions_enabled: consumer_interactions,
        native_media_enabled: native_media,
        relay_text_edits_enabled: relay_text_edits,
        native_discord_edits_enabled: native_discord_edits || discord_card_edits,
        discord_effects_issuance_enabled: native_discord_effects
            && (native_discord_edits || native_discord_media_effects),
        discord_effects: Some(std::sync::Arc::new(
            tools::DiscordEffectsClient::new(crate::discord::DiscordRest::new()?)
                .with_card_effects(discord_card_effects),
        )),
        lark_cards_enabled: lark_cards,
        resolver: state.resolver.clone(),
        keys: state.master_keys.clone(),
        providers,
        delivery,
        tracker: tracker.clone(),
        public_origin: config.management.public_url.clone(),
        public_api_url: config.management.public_api_url.clone(),
        advertise_public_api_url: config.management.advertise_public_api_url,
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
    let mut routed_worker = routed.map(|composition| {
        crate::serve_routed::Worker::start(composition.scheduler, state.readiness.clone())
    });
    let mut native_discord = crate::discord::lifecycle::Worker::start(
        &config.native_discord,
        state.native_discord.clone(),
        native_discord_threads,
        (*state.records).clone(),
        state.master_keys.clone(),
        ingress.clone(),
        state.readiness.clone(),
    )
    .map_err(|_| std::io::Error::other("native Discord runtime configuration is invalid"))?;
    tracing::info!(bind = %config.listen, "CMAEG listening");

    let (shutdown_started, shutdown_signal) = oneshot::channel();
    let mut router = router_with_state(state)
        .merge(http::events::router(http::events::CustomIngressState {
            ingress: ingress.clone(),
            waiters,
        }))
        .merge(http::connections::router(connections.clone()))
        .merge(http::relay_resource_connections::router(
            resource_connections,
        ))
        .merge(http::connection_browser::router(connections))
        .merge(http::whatsapp::router(ingress.clone()))
        .merge(http::telegram::router(ingress.clone()))
        .merge(http::callbacks::router(ingress))
        .merge(http::tools::router(tools_state.clone()))
        .merge(http::media::router(tools_state.clone()))
        .merge(mcp::router(tools_state))
        .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES))
        .layer(CatchPanicLayer::custom(http::problems::panic_response));
    if let Some(browser) = browser {
        router = router.merge(cma_service_auth::session::router(browser));
    }
    // The retained console has its own OAuth session, CSRF and owner checks.
    // Portal cookie middleware must not change that independent authority.
    router = router.merge(console.unwrap_or_default());
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        wait_for_shutdown_signal().await;
        tracker.start_draining();
        let deadline = Instant::now() + config.turns.drain;
        if let Some(worker) = &native_discord {
            worker.stop();
        }
        if let Some(worker) = &routed_worker {
            worker.stop_scheduling();
        }
        let (turns_drained, routed_drained, native_drained) = tokio::join!(
            tracker.drain(deadline),
            async {
                match &mut routed_worker {
                    Some(worker) => worker.drain(deadline).await,
                    None => true,
                }
            },
            async {
                match &mut native_discord {
                    Some(worker) => worker.drain(deadline).await,
                    None => true,
                }
            }
        );
        if !native_drained {
            tracing::warn!(
                "native Discord drain incomplete; durable inbox and lease fences retained"
            );
        }
        if !turns_drained {
            tracing::warn!("turn drain deadline reached; remaining consumers cancelled");
        }
        if !routed_drained {
            tracing::warn!("routed execution drain incomplete; durable recovery retained");
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
pub(crate) async fn record_client(
    config: &Config,
    service_auth: service_auth::ServiceAuth,
) -> Result<records::RecordClient, records::RecordError> {
    let client = records::RecordClient::new(
        &config.management.cma_base_url,
        config.management.cma_service_token.as_str(),
    )?
    .with_service_auth(service_auth);
    crate::serve_storage::record_client(client).await
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
