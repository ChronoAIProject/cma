//! Compose CMAEG and drain turns, deliveries and connections on termination.
use std::{future::IntoFuture, net::SocketAddr, sync::Arc};
use tokio::sync::oneshot;
use tower_http::{catch_panic::CatchPanicLayer, limit::RequestBodyLimitLayer};

use crate::{
    connections, console, delivery, http, ingress, interactions, media, providers,
    router_with_state, service_auth, telegram, telemetry, tools, turns, Config,
    REQUEST_BODY_LIMIT_BYTES,
};

/// Compose CMAEG from `config` and run it until a process termination signal arrives.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve(
    mut config: Config,
    complete_replies: bool,
    commonmark_relay: crate::config::CommonmarkRelay,
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
    telegram_media_effects: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let replicas = crate::replicas::config::Config::from_env()?;
    config.console.validate_replica_mode(replicas.enabled)?;
    let http_listener = listener::Http::prepare(&mut config).await?;
    let discord_media_edits = config.native_discord_media_card_edits_enabled && discord_card_edits;
    let discord_media_replacements =
        config.native_discord_media_card_replacements_enabled && discord_media_edits;
    let service_auth = service_auth::ServiceAuth::from_config(&config)?;
    let request_audit = crate::request_audit::Writer::configured(
        &config.management.cma_base_url,
        service_auth.clone(),
    )?;
    // Prove the renewable credential before advertising gateway readiness.
    service_auth.bearer().await?;
    let (records, creator_auth, browser) =
        crate::serve_storage::compose(&config, service_auth.clone()).await?;
    let console_sessions = console::prepare_sessions(&config.console, &records).await?;
    // Install before constructing instrumented pools and queues.
    let metrics = telemetry::install()?;
    let providers = Arc::new(
        providers::ProviderClient::from_config(&config)?
            .with_service_auth(service_auth)
            .with_worker_credentials(records.worker_credentials()),
    );
    let resource_connections = connections::relay_resources::ResourceConnections::new(
        creator_auth.clone(),
        &config.management.nyxid_base_url,
    )?;
    let mut observers = console::observer::Observers::new(config.console.enabled);
    let mut state = http::ManagementState::observed(
        providers.clone(),
        records,
        creator_auth,
        config.management.clone(),
        config.channel_cache.clone(),
        observers.index.clone(),
    );
    crate::channels::profile_revision::configure(&config, &mut state)?;
    activity::configure(&config, &mut state, &mut observers).await?;
    let telegram = Arc::new(telegram::TelegramClient::production(
        config.delivery.reply_timeout,
    )?);
    state.telegram = Some(telegram.clone());
    state.media_enabled = config.media.enabled;
    let whatsapp_forms_ready = source_settings::configure(&config, &mut state).await?;
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
    state.whatsapp = Some(Arc::new(whatsapp.clone()));
    let whatsapp_receipts = Arc::new(crate::whatsapp::receipts::Store::new(
        (*state.records).clone(),
        state.master_keys.clone(),
    ));
    let _reachability_probe = state.readiness.start_probe(state.records.clone());
    let tracker = turns::TurnTracker::new(state.readiness.clone());
    let drain_control = crate::replicas::drain::Control::new(
        state.master_keys.clone(),
        tracker.clone(),
        config.listen.port(),
        &config.management.public_url,
    );
    let replica_runtime = http_listener
        .start_replicas(replicas.enabled, &config, &state, &drain_control)
        .await?;
    let (scheduler, outcomes) = delivery::router::ShardedScheduler::channel(256);
    let delivery: std::sync::Arc<dyn delivery::router::DeliverySink> = Arc::new(scheduler);
    let registry = delivery::sync::SyncRegistry::default();
    let registry = match &replica_runtime {
        Some(runtime) => registry.with_instance(runtime.partitions.membership().instance),
        None => registry,
    };
    let waiters = Arc::new(registry);
    let custom_delivery = delivery::custom::CustomDelivery::new(
        config.management.public_url.clone(),
        waiters.clone(),
    )
    .with_activity(observers.activity.clone());
    let interactions = Arc::new(interactions::InteractionStore::new(
        (*state.records).clone(),
        state.master_keys.clone(),
        providers.clone(),
    ));
    let whatsapp_forms = Arc::new(crate::whatsapp::forms::service::Forms::new(
        (*state.records).clone(),
        state.master_keys.clone(),
        state.resolver.clone(),
        interactions.as_ref().clone(),
        whatsapp.clone(),
        state.native_whatsapp_forms_enabled && state.native_whatsapp_admission_enabled,
        state.native_whatsapp_flow_writes_enabled && state.native_whatsapp_admission_enabled,
    ));
    let native_typing = source_settings::native_typing(&config, &state)?;
    let (native_lark_controls, native_lark_thread_start) =
        source_settings::lark_issuance(&config, &state);
    let tool_providers = Arc::new(config.media.tool_providers.clone());
    // The recorder is installed before the Console registers its gauge.
    let console =
        console::compose(&config, &state, observers.queue.take(), console_sessions).await?;
    // Before the listener binds: a configured transformer must pass its
    // sandbox self-check, or the gateway does not start.
    let transformer = media::transform::setup::start_configured(&config).await?;
    // Inbound media and outbound voice share one transformer and its budgets.
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
    let delivery_engine = delivery::engine::DeliveryEngine::start_observed(
        outcomes,
        state.master_keys.clone(),
        delivery::relay_client::RelayClient::new(
            &config.management.nyxid_base_url,
            config.delivery.reply_timeout,
        )?
        .with_native_lark(
            crate::native_lark::Client::production()?,
            Arc::new(crate::native_lark::receipts::Store::new(
                (*state.records).clone(),
                state.master_keys.clone(),
            )),
        )
        .with_whatsapp(whatsapp, whatsapp_receipts.clone())
        .with_native_split_text_refs(config.native_split_text_refs_enabled)
        .with_native_media_edits(
            config.native_telegram_media_edits_enabled,
            config.native_discord_media_edits_enabled,
            Arc::new(crate::native_media::store::Store::new(
                (*state.records).clone(),
                state.master_keys.clone(),
            )),
        )
        .with_native_lark_config(&config, native_lark_controls, native_lark_thread_start)
        .with_native_presentation_config(&config)
        .with_discord_voice_decoder(media.as_ref().and_then(|service| service.voice_decoder()))
        .with_native_whatsapp_config(&config)
        .with_complete_replies(complete_replies)
        .with_commonmark_platforms(commonmark_relay)
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
        .with_discord_media_controls(config.native_discord_media_controls_enabled)
        .with_discord_media_card_edits(discord_media_edits)
        .with_discord_media_card_replacements(discord_media_replacements)
        .with_discord_media_card_sets(config.native_discord_media_card_sets_enabled)
        .with_discord_card_edits(discord_card_edits)
        .with_consumer_interactions(consumer_interactions)
        .with_telegram_effects(telegram_effects)
        .with_telegram_media_effects(telegram_media_effects)
        .with_telegram_media_controls(config.native_telegram_media_controls_enabled)
        .with_telegram_media_control_edits(config.native_telegram_media_control_edits_enabled)
        .with_interactions(interactions.clone()),
        config.delivery.clone(),
        tracker.clone(),
        custom_delivery.clone(),
        observers.activity.clone(),
    );
    let routed = crate::serve_routed::compose(
        &config,
        &mut state,
        crate::serve_routed::Deps {
            media: media.clone(),
            tool_providers: tool_providers.clone(),
            delivery: delivery.clone(),
            tracker: tracker.clone(),
            custom: custom_delivery,
            partitions: replica_runtime
                .as_ref()
                .map(|runtime| runtime.partitions.clone()),
        },
    )
    .await?;
    let runner = Arc::new(
        turns::TurnRunner::new(
            providers.clone(),
            tracker.clone(),
            delivery.clone(),
            config.turns.clone(),
        )
        .with_activity(observers.activity.clone())
        .with_recovery_authority(Arc::new(state.resolver.clone()))
        .with_media_tool_providers(tool_providers)
        .with_media(
            media.clone(),
            media::prepare::PrepareConfig {
                budget: config.media.prepare_budget,
                eager_audio_max_seconds: config.stt.eager_max_seconds,
            },
        ),
    );
    let lark_effects = source_settings::lark_effects(&config, native_lark_controls)?;
    let ingress = ingress::IngressState::new(
        state.resolver.clone(),
        state.master_keys.clone(),
        config.ingress.clone(),
        Arc::new(turns::observer::ObservedLauncher::new(
            runner.clone(),
            observers.activity.clone(),
        )),
    )?
    .with_profile_revisions(config.profile_revisions_enabled)
    .with_whatsapp_receipts(whatsapp_receipts)
    .with_telegram(telegram.clone())
    .with_routing_notice_relay(state.outbound_relay.clone())
    .with_telegram_effects(telegram_effects || config.native_telegram_received_reactions_enabled)
    .with_interactions(interactions)
    .with_native_intake_config(&config)
    .with_routed(
        routed
            .as_ref()
            .map(|composition| composition.ingress.clone()),
    )
    .with_activity_observer(observers.activity.clone());
    let tools_state = tools::ToolsState {
        native_typing,
        native_lark_conversation_history: source_settings::lark_history(
            config.native_lark_conversation_history_enabled,
            config.native_lark_mixed_history_enabled,
        )?,
        discord_conversation_history: source_settings::discord_history(
            config.native_discord_conversation_history_enabled,
        )?,
        telegram_topics: source_settings::telegram_topics(&state, telegram.clone()),
        discord_threads: state
            .native_discord_thread_lifecycle_enabled
            .then(|| crate::discord::DiscordRest::new().map(tools::DiscordThreads::new))
            .transpose()?
            .map(Arc::new),
        discord_thread_history: config
            .native_discord_thread_history_enabled
            .then(|| crate::discord::DiscordRest::new().map(tools::DiscordThreadHistory::new))
            .transpose()?
            .map(Arc::new),
        complete_replies_enabled: complete_replies,
        commonmark_relay,
        routed_replies: routed
            .as_ref()
            .map(|composition| composition.tool_replies.clone()),
        telegram_effects_issuance_enabled: telegram_effects,
        native_telegram_media_controls_enabled: config.native_telegram_media_controls_enabled,
        native_telegram_media_control_edits_enabled: config
            .native_telegram_media_control_edits_enabled,
        native_media_edits: Some(Arc::new(crate::native_media::Client::new(
            telegram.clone(),
            crate::discord::DiscordRest::new()?,
            config.native_telegram_media_edits_enabled,
            config.native_discord_media_edits_enabled,
        ))),
        native_whatsapp_forms: Some(whatsapp_forms.clone()),
        native_lark_effects: Some(lark_effects),
        lark_effects_issuance_enabled: config.lark_message_effects_writes_enabled,
        lark_effects: Some(Arc::new(
            tools::message_effects::lark_transport::LarkEffectsClient::new(
                &config.management.nyxid_base_url,
            )?,
        )),
        telegram_effects: Some(Arc::new(
            tools::MessageEffectsClient::new(telegram)
                .with_received_reactions(config.native_telegram_received_reactions_enabled),
        )),
        whatsapp_templates_enabled: whatsapp_templates,
        discord_interactions_enabled: discord_interactions,
        native_discord_media_controls_enabled: config.native_discord_media_controls_enabled,
        native_discord_media_card_edits_enabled: discord_media_edits,
        native_discord_media_card_replacements_enabled: discord_media_replacements,
        native_discord_media_card_sets_enabled: config.native_discord_media_card_sets_enabled,
        whatsapp_interactions_enabled: whatsapp_interactions,
        native_whatsapp_interactions_enabled: config.native_whatsapp_interactions_enabled,
        native_whatsapp_media_controls_enabled: config.native_whatsapp_media_controls_enabled,
        consumer_interactions_enabled: consumer_interactions,
        native_media_enabled: native_media,
        native_lark_outbound_media_enabled: config.native_lark_outbound_media_enabled,
        native_lark_sticker_replies_enabled: config.native_lark_sticker_replies_enabled,
        native_lark_thread_start_enabled: native_lark_thread_start,
        native_lark_media_controls_enabled: config.native_lark_media_controls_enabled
            && native_lark_controls,
        native_lark_multi_image_controls_enabled: config.native_lark_multi_image_controls_enabled,
        native_lark_media_card_replacements_enabled: config
            .native_lark_media_card_replacements_enabled,
        native_media_presentations_enabled: config.native_media_presentations_enabled,
        native_discord_voice_enabled: config.native_discord_voice_enabled,
        native_telegram_structured_replies_enabled: config
            .native_telegram_structured_replies_enabled,
        native_whatsapp_structured_replies_enabled: config
            .native_whatsapp_structured_replies_enabled
            && config.native_whatsapp_writes_enabled,
        native_telegram_albums_enabled: config.native_telegram_albums_enabled,
        native_whatsapp_outbound_media_enabled: config.native_whatsapp_outbound_media_enabled,
        whatsapp_reactions: Some(Arc::new(tools::WhatsappReactionsClient::new(
            crate::whatsapp::Client::production()?,
            config.native_whatsapp_reactions_enabled,
        ))),
        native_whatsapp_templates_enabled: config.native_whatsapp_templates_enabled,
        native_whatsapp_groups_enabled: config.native_whatsapp_groups_enabled,
        relay_text_edits_enabled: relay_text_edits,
        native_discord_edits_enabled: native_discord_edits || discord_card_edits,
        discord_effects_issuance_enabled: native_discord_effects
            && (native_discord_edits || native_discord_media_effects),
        discord_effects: Some(Arc::new(
            tools::DiscordEffectsClient::new(crate::discord::DiscordRest::new()?)
                .with_card_effects(discord_card_effects)
                .with_received_reactions(config.native_discord_received_reactions_enabled),
        )),
        lark_cards_enabled: lark_cards,
        resolver: state.resolver.clone(),
        keys: state.master_keys.clone(),
        providers,
        delivery,
        tracker: tracker.clone(),
        public_origin: config.management.public_url.clone(),
        public_api_url: config.management.public_api_url.clone(),
        public_api_aliases: config.management.public_api_aliases.clone(),
        advertise_public_api_url: config.management.advertise_public_api_url,
        media,
        downloads: Arc::new(
            tools::MediaDownloads::default().with_link_ttl(config.media.link_ttl_seconds),
        ),
    };
    let connections = connections::ConnectionService::new(state.clone(), tools_state.clone());
    let completion_launcher = if replicas.enabled {
        routed
            .as_ref()
            .ok_or("replica completion requires durable routed admission")?
            .ingress
            .replica_completion_launcher(observers.activity.clone())
    } else {
        runner
    };
    let _connection_worker = connections.start_completion_worker(completion_launcher);
    let (listener, ephemeral) = http_listener.bind(config.listen).await?;
    let github_initialization = config
        .github_enabled
        .then(|| crate::github::Initialization::start(state.clone(), ingress.clone()));
    let ingress = ingress.with_receipt_observer(Arc::new(connections.clone()));
    let metrics =
        crate::serve_routed::start_metrics(config.metrics_listen, metrics, routed.as_ref()).await?;
    let replica_routes = routed
        .as_ref()
        .map(|composition| composition.replica_routes.clone())
        .unwrap_or_default();
    let routed_workers = crate::serve_routed::start_workers(routed, state.readiness.clone());
    // Readers and recovery remain active when controls/forms issuance is disabled.
    let native_lark_controls = crate::native_lark::control_recovery::Worker::start(
        ingress.clone(),
        state.readiness.clone(),
    );
    let whatsapp_form_worker = whatsapp_forms_ready.then(|| {
        crate::whatsapp::forms::worker::Worker::start(
            ingress.clone(),
            whatsapp_forms.provisioner.clone(),
            state.readiness.clone(),
        )
    });
    let native_discord = crate::discord::lifecycle::Worker::start(
        &config.native_discord,
        state.native_discord.clone(),
        native_discord_threads,
        (*state.records).clone(),
        state.master_keys.clone(),
        ingress.clone(),
        state.readiness.clone(),
        replica_runtime
            .as_ref()
            .map(|runtime| runtime.partitions.membership()),
    )
    .map_err(|_| std::io::Error::other("native Discord runtime configuration is invalid"))?;
    listener::announce(&listener, metrics.address(), ephemeral)?;
    tracing::info!(bind = %config.listen, "CMAEG listening");
    let (shutdown_started, shutdown_signal) = oneshot::channel();
    let side_messages = delivery_engine.side_messages_if(config.event_side_messages_enabled);
    let mut router = router_with_state(state)
        .merge(drain_control.router(replicas.enabled))
        .merge(replica_routes)
        .merge(http::events::router(http::events::CustomIngressState {
            ingress: ingress.clone(),
            waiters,
        }))
        .merge(http::connections::router(connections.clone()))
        .merge(http::relay_resource_connections::router(
            resource_connections,
        ))
        .merge(http::connection_browser::router(connections))
        .merge(http::native_lark::router(ingress.clone()))
        .merge(http::whatsapp::router(ingress.clone()))
        .merge(http::telegram::router(ingress.clone()))
        .merge(http::callbacks::router(ingress))
        .merge(http::tools::gateway_router(tools_state, side_messages))
        .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES))
        .layer(CatchPanicLayer::custom(http::problems::panic_response));
    if let Some(browser) = browser {
        router = router.merge(cma_service_auth::session::router(browser));
    }
    // The retained console has its own OAuth session, CSRF and owner checks.
    // Portal cookie middleware must not change that independent authority.
    router = router.merge(console.unwrap_or_default());
    if let Some(writer) = request_audit {
        router = router.layer(axum::middleware::from_fn_with_state(
            writer,
            crate::request_audit::audit,
        ));
    }
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! {
            _ = shutdown::wait_for_shutdown_signal() => drain_control.start(),
            _ = drain_control.requested() => {}
        }
        let deadline = shutdown::Drain {
            tracker,
            drain_timeout: config.turns.drain,
            native_lark_controls,
            whatsapp_form_worker,
            native_discord,
            routed_workers,
            github_initialization,
            delivery_engine,
        }
        .run()
        .await;
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

#[path = "serve_listener.rs"]
mod listener;
#[path = "serve_shutdown.rs"]
mod shutdown;

#[path = "serve_activity.rs"]
mod activity;

#[path = "serve_record_client.rs"]
mod record_client_impl;
pub(crate) use record_client_impl::record_client;

#[path = "serve_source_settings.rs"]
mod source_settings;
