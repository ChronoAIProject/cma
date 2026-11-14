//! Start serving only after warm-pool and retained-runtime authority are ready.
use std::{future::IntoFuture, net::SocketAddr, time::Duration};

use crate::{
    agent_manager,
    composition::{self, ControlPlaneComposition},
    config::Config,
    read_telemetry,
    shutdown::{self, signalled, ShutdownCoordinator},
    telemetry, with_cors, with_request_context,
};

// Match the Gateway's longest accepted connection before draining workers.
const HTTP_DRAIN_TIMEOUT: Duration = Duration::from_secs(660);

pub(crate) async fn serve() -> Result<(), String> {
    let config =
        Config::from_env().map_err(|error| format!("configuration is not usable: {error}"))?;
    if std::env::args().nth(1).as_deref() == Some("--agent-manager-drain") {
        return agent_manager::relay::prestop(&config).await;
    }
    let recorder = telemetry::install()?;
    let _metrics_server = telemetry::server::Server::start(config.metrics_bind, recorder).await?;
    let composition = ControlPlaneComposition::build(&config).await?;
    let auth_mode = config.auth_mode_name();
    let app = with_request_context(with_cors(composition.router, config.cors_origins));

    // Do not admit allocations against a newly promoted runtime until its
    // exact default pool has ready capacity (unless an operator disabled it).
    composition
        .native_warm_pools
        .ensure_default()
        .await
        .map_err(|error| format!("native warm pool default is not usable: {error:?}"))?;
    composition
        .native_warm_pools
        .await_default_ready()
        .await
        .map_err(|error| format!("native warm pool startup readiness failed: {error:?}"))?;

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|error| format!("cannot bind {}: {error}", config.bind))?;
    composition.sandbox_lifecycle.initialize().await?;
    tracing::info!(
        bind = %config.bind,
        public_url = %config.public_url,
        auth_mode,
        "CMA listening"
    );
    if let Some(manager) = &composition.agent_manager {
        manager.activate()?;
    }
    let mut shutdown = ShutdownCoordinator::new();
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
    let (http_stop, http_signal) = tokio::sync::watch::channel(false);
    let server_result = {
        let server = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(signalled(http_signal))
        .into_future();
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

