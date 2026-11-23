//! Start serving only after warm-pool and retained-runtime authority are ready.
use std::{
    future::IntoFuture,
    net::SocketAddr,
    time::{Duration, Instant},
};

use crate::{
    agent_manager,
    composition::{self, ControlPlaneComposition},
    config::StartupConfig,
    read_telemetry,
    shutdown::{self, signalled, ShutdownCoordinator},
    startup_timing::{observe, Phase},
    telemetry, with_cors, with_request_context,
};

// Match the Gateway's longest accepted connection before draining workers.
const HTTP_DRAIN_TIMEOUT: Duration = Duration::from_secs(660);

pub(crate) async fn serve() -> Result<(), String> {
    let startup_started = Instant::now();
    let config = StartupConfig::from_env()
        .map_err(|error| format!("configuration is not usable: {error}"))?;
    let config = match config {
        StartupConfig::Execution(config) => *config,
        StartupConfig::BackendFree(config) => {
            return composition::backend_free::serve(*config).await
        }
    };
    if std::env::args().nth(1).as_deref() == Some("--agent-manager-drain") {
        return agent_manager::relay::prestop(&config).await;
    }
    let recorder = telemetry::install()?;
    let _metrics_server = telemetry::server::Server::start(config.metrics_bind, recorder).await?;
    let composition = observe(
        "composition",
        ControlPlaneComposition::build(&config),
        |_| None,
    )
    .await?;
    let auth_mode = config.auth_mode_name();
    let app = with_request_context(with_cors(composition.router, config.cors_origins)).layer(
        axum::Extension(telemetry::http::ContextDiagnosticsEnabled(
            config.features.context_diagnostics,
        )),
    );

    // Do not admit allocations against a newly promoted runtime until its
    // exact default pool has ready capacity (unless an operator disabled it).
    // The shared desktop pool is seeded here but fills in the background.
    let mut shutdown = ShutdownCoordinator::new();
    let (runtime_writer_ready, booting) = tokio::sync::watch::channel(false);
    if let Some(controller) = &composition.runtime_pointer {
        controller.register_boot().await?;
        shutdown.push(controller.spawn_heartbeat(booting, shutdown.subscribe()));
        if !controller.following_configured() {
            controller
                .set_following(false)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    observe(
        "default_warm_pool_configuration",
        async {
            match &composition.runtime_pointer {
                Some(controller) => controller.bootstrap().await,
                None => composition
                    .native_warm_pools
                    .ensure_default()
                    .await
                    .map_err(|error| format!("native warm pool default is not usable: {error:?}")),
            }
        },
        |_| None,
    )
    .await?;
    observe(
        "default_warm_pool_readiness",
        async {
            composition
                .native_warm_pools
                .await_default_ready()
                .await
                .map_err(|error| format!("native warm pool startup readiness failed: {error:?}"))
        },
        |_| None,
    )
    .await?;

    let listener = observe(
        "socket_bind",
        tokio::net::TcpListener::bind(config.bind),
        |_| Some(1),
    )
    .await
    .map_err(|error| format!("cannot bind {}: {error}", config.bind))?;
    observe(
        "sandbox_initialization",
        composition.sandbox_lifecycle.initialize(),
        |_| None,
    )
    .await?;
    tracing::info!(
        target: "cma",
        bind = %config.bind,
        public_url = %config.public_url,
        auth_mode,
        "CMA listening"
    );
    let workers = Phase::new("worker_start");
    if let Some(manager) = &composition.agent_manager {
        if let Err(error) = manager.activate().await {
            workers.finish("error", None);
            return Err(error);
        }
    }
    runtime_writer_ready.send_replace(true);
    if let Some(controller) = composition.runtime_pointer {
        shutdown.push(controller.spawn(shutdown.subscribe()));
    }
    composition::activity::spawn(composition.activity_worker, &mut shutdown);
    shutdown.push(read_telemetry::spawn(shutdown.subscribe()));
    shutdown.push(
        composition
            .auth
            .spawn_actor_link_renewal(shutdown.subscribe()),
    );
    shutdown.push(composition.bindings.spawn(shutdown.subscribe()));
    if let Some(bots) = composition.bots {
        shutdown.push(bots.spawn(shutdown.subscribe()));
    }
    if let Some(continuations) = composition.actor_continuations {
        shutdown.push(continuations.spawn(shutdown.subscribe()));
    }
    let worker_signal = shutdown.subscribe();
    shutdown.push(
        composition
            .launch_scheduler
            .spawn_supervised(worker_signal, composition.workers.get("launches")),
    );
    shutdown.push(composition.command_scheduler.spawn_supervised(
        shutdown.subscribe(),
        composition.workers.get("agent_commands"),
    ));
    shutdown.push(composition.sandbox_control_scheduler.spawn_supervised(
        shutdown.subscribe(),
        composition.workers.get("sandbox_commands"),
    ));
    shutdown.push(composition.worker_observations.spawn(shutdown.subscribe()));
    shutdown.push(
        composition
            .native_warm_pools
            .clone()
            .spawn_observations(shutdown.subscribe()),
    );
    shutdown.push(composition.native_warm_pools.spawn_supervised(
        shutdown.subscribe(),
        composition.workers.get("native_warm_pools"),
    ));
    shutdown.push(composition.native_prepared.spawn(shutdown.subscribe()));
    shutdown.push(composition.agent_access_worker.spawn(shutdown.subscribe()));
    composition
        .sandbox_lifecycle
        .spawn(&mut shutdown, &composition.workers);
    shutdown.push(composition.provenance_backfill.spawn(shutdown.subscribe()));
    composition.usage.spawn(&mut shutdown);
    composition.usage_metrics.spawn(&mut shutdown);
    workers.finish("ok", None);
    // This is entry into HTTP serving, not an observed HTTP/Kubernetes Ready result.
    tracing::info!(target: "cma_startup", parent: None,
        phase = "serving_entry", elapsed_ms = startup_started.elapsed().as_millis(),
        timing_basis = "serve_entry", outcome = "ok", "API startup serving entry");
    let (http_stop, http_signal) = tokio::sync::watch::channel(false);
    let server_result = {
        let server: std::pin::Pin<
            Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>,
        > = match config.http_connection_max_age {
            // Bounded connection age lets long-lived clients rebalance across replicas.
            Some(age) => Box::pin(crate::http_serving::serve(
                listener,
                app,
                age,
                signalled(http_signal),
            )),
            None => Box::pin(
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(signalled(http_signal))
                .into_future(),
            ),
        };
        tokio::pin!(server);
        tokio::select! {
            result = &mut server => result.map_err(|error| format!("HTTP server failed: {error}")),
            () = async {
                match &composition.agent_manager {
                    Some(manager) => tokio::select! { _ = shutdown::shutdown_signal() => (), _ = manager.failed() => () },
                    None => shutdown::shutdown_signal().await,
                }
            } => {
                let manager_result = shutdown.drain_agent_manager(composition.agent_manager.as_deref()).await;
                // Existing streams still need their workers until HTTP finishes.
                let _ = http_stop.send(true);
                let http_result = match tokio::time::timeout(HTTP_DRAIN_TIMEOUT, &mut server).await {
                    Ok(result) => result.map_err(|error| format!("HTTP server failed: {error}")),
                    Err(_) => Err("HTTP server exceeded its shutdown drain deadline".into()),
                };
                manager_result.and(http_result)
            }
        }
    };
    let manager_result = shutdown
        .drain_agent_manager(composition.agent_manager.as_deref())
        .await;
    let server_result = server_result.and(manager_result);
    let aborted = shutdown.drain().await;
    match (server_result, aborted) {
        (Err(error), _) => Err(error),
        (Ok(()), 0) => Ok(()),
        (Ok(()), count) => Err(format!(
            "{count} background workers exceeded shutdown drain"
        )),
    }
}
