//! Initialize execution authority and manage the API listener.
mod admission;
mod early;
pub(crate) use admission::Admission;
use std::{
    future::{Future, IntoFuture},
    net::SocketAddr,
    pin::Pin,
    time::{Duration, Instant},
};

use axum::Router;
use tokio::{net::TcpListener, sync::watch};

use crate::{
    agent_manager,
    composition::{self, ControlPlaneComposition},
    config::{Config, StartupConfig},
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
    let mut config = match config {
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
    let mut composition = observe(
        "composition",
        ControlPlaneComposition::build(&config),
        |_| None,
    )
    .await?;
    let app = with_request_context(with_cors(
        std::mem::take(&mut composition.router),
        std::mem::take(&mut config.cors_origins),
    ))
    .layer(axum::Extension(telemetry::http::ContextDiagnosticsEnabled(
        config.features.context_diagnostics,
    )));
    if config.features.am_startup_health_serving {
        return early::serve(&config, composition, app, startup_started).await;
    }
    serve_after_initialization(&config, composition, app, startup_started).await
}

async fn warm_pools(
    composition: &ControlPlaneComposition,
    admission: Option<&Admission>,
) -> Result<(), String> {
    // Do not admit allocations against a newly promoted runtime until its
    // exact default pool has ready capacity (unless an operator disabled it).
    // The shared desktop pool is seeded here but fills in the background.
    observe(
        "default_warm_pool_configuration",
        composition.native_warm_pools.ensure_default(),
        |_| None,
    )
    .await
    .map_err(|error| format!("native warm pool default is not usable: {error:?}"))?;
    if admission.is_some_and(Admission::is_draining) {
        return Ok(());
    }
    observe(
        "default_warm_pool_readiness",
        composition
            .native_warm_pools
            .await_default_ready_until(admission),
        |_| None,
    )
    .await
    .map_err(|error| format!("native warm pool startup readiness failed: {error:?}"))
}

async fn bind(address: SocketAddr) -> Result<TcpListener, String> {
    observe("socket_bind", TcpListener::bind(address), |_| Some(1))
        .await
        .map_err(|error| format!("cannot bind {address}: {error}"))
}

async fn initialize_sandboxes(
    composition: &ControlPlaneComposition,
    admission: Option<&Admission>,
) -> Result<(), String> {
    observe(
        "sandbox_initialization",
        composition
            .sandbox_lifecycle
            .initialize(|| admission.is_some_and(Admission::is_draining)),
        |_| None,
    )
    .await
}

fn listening(config: &Config) {
    tracing::info!(
        target: "cma",
        bind = %config.bind,
        public_url = %config.public_url,
        auth_mode = config.auth_mode_name(),
        "CMA listening"
    );
}

fn serving_entry(startup_started: Instant) {
    // Entry into HTTP execution serving is separate from an observed Ready response.
    tracing::info!(target: "cma_startup", parent: None,
        phase = "serving_entry", elapsed_ms = startup_started.elapsed().as_millis(),
        timing_basis = "serve_entry", outcome = "ok", "API startup serving entry");
}

async fn activate_and_start_workers(
    composition: ControlPlaneComposition,
    shutdown: &mut ShutdownCoordinator,
    admission: Option<&Admission>,
) -> Result<(), String> {
    let workers = Phase::new("worker_start");
    if let Some(manager) = &composition.agent_manager {
        if let Err(error) = manager.activate().await {
            workers.finish("error", None);
            return Err(error);
        }
    }
    if admission.is_some_and(Admission::is_draining) {
        workers.finish("stopped", None);
        return Ok(());
    }
    start_workers(composition, shutdown);
    workers.finish("ok", None);
    Ok(())
}

fn start_workers(composition: ControlPlaneComposition, shutdown: &mut ShutdownCoordinator) {
    composition::activity::spawn(composition.activity_worker, shutdown);
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
        .spawn(shutdown, &composition.workers);
    shutdown.push(composition.provenance_backfill.spawn(shutdown.subscribe()));
    composition.usage.spawn(shutdown);
    composition.usage_metrics.spawn(shutdown);
}

type HttpServer = Pin<Box<dyn Future<Output = std::io::Result<()>> + Send>>;

fn http_server(
    listener: TcpListener,
    app: Router,
    max_age: Option<Duration>,
    http_signal: watch::Receiver<bool>,
) -> HttpServer {
    match max_age {
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
    }
}

fn http_result(result: std::io::Result<()>) -> Result<(), String> {
    result.map_err(|error| format!("HTTP server failed: {error}"))
}

async fn drain_http(stop: watch::Sender<bool>, mut server: HttpServer) -> Result<(), String> {
    // Existing streams still need their workers until HTTP finishes.
    stop.send_replace(true);
    match tokio::time::timeout(HTTP_DRAIN_TIMEOUT, &mut server).await {
        Ok(result) => http_result(result),
        Err(_) => Err("HTTP server exceeded its shutdown drain deadline".into()),
    }
}

async fn serve_after_initialization(
    config: &Config,
    composition: ControlPlaneComposition,
    app: Router,
    startup_started: Instant,
) -> Result<(), String> {
    warm_pools(&composition, None).await?;
    let listener = bind(config.bind).await?;
    initialize_sandboxes(&composition, None).await?;
    listening(config);
    let manager = composition.agent_manager.clone();
    let mut shutdown = ShutdownCoordinator::new();
    activate_and_start_workers(composition, &mut shutdown, None).await?;
    serving_entry(startup_started);
    let (http_stop, http_signal) = watch::channel(false);
    let mut server = http_server(listener, app, config.http_connection_max_age, http_signal);
    let server_result = tokio::select! {
        result = &mut server => http_result(result),
        () = async {
            match &manager {
                Some(manager) => tokio::select! { _ = shutdown::shutdown_signal() => (), _ = manager.failed() => () },
                None => shutdown::shutdown_signal().await,
            }
        } => {
            let manager_result = shutdown.drain_agent_manager(manager.as_deref()).await;
            manager_result.and(drain_http(http_stop, server).await)
        }
    };
    let manager_result = shutdown.drain_agent_manager(manager.as_deref()).await;
    finish(shutdown, server_result.and(manager_result)).await
}

async fn finish(
    shutdown: ShutdownCoordinator,
    server_result: Result<(), String>,
) -> Result<(), String> {
    let aborted = shutdown.drain().await;
    match (server_result, aborted) {
        (Err(error), _) => Err(error),
        (Ok(()), 0) => Ok(()),
        (Ok(()), count) => Err(format!(
            "{count} background workers exceeded shutdown drain"
        )),
    }
}
