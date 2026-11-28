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
    composition::{self, ControlPlaneComposition, StartupComposition},
    config::{Config, StartupConfig},
    image_manager_retirement, read_telemetry,
    shutdown::{self, signalled, ShutdownCoordinator},
    startup_timing::{observe, observe_managed_machine_scope, Phase},
    telemetry,
    warm_pools::{startup::WarmPoolStartup, NativeWarmPools},
    with_cors, with_request_context,
};

// Match the Gateway's longest accepted connection before draining workers.
const HTTP_DRAIN_TIMEOUT: Duration = Duration::from_secs(660);

pub(crate) async fn serve() -> Result<(), String> {
    serve_inner(
        #[cfg(feature = "qualification-hooks")]
        None,
    )
    .await
}

#[cfg(feature = "qualification-hooks")]
pub(crate) async fn serve_qualified(options: crate::qualification::Options) -> Result<(), String> {
    serve_inner(Some(options)).await
}

async fn serve_inner(
    #[cfg(feature = "qualification-hooks")] qualification: Option<crate::qualification::Options>,
) -> Result<(), String> {
    let startup_started = Instant::now();
    let config = StartupConfig::from_env()
        .map_err(|error| format!("configuration is not usable: {error}"))?;
    let mut config = match config {
        StartupConfig::Execution(config) => *config,
        StartupConfig::BackendFree(config) => {
            #[cfg(feature = "qualification-hooks")]
            if qualification.is_some() {
                return Err("qualification requires an execution daemon".into());
            }
            return composition::backend_free::serve(*config).await;
        }
    };
    if std::env::args().nth(1).as_deref() == Some("--agent-manager-drain") {
        return agent_manager::relay::prestop(&config).await;
    }
    observe_managed_machine_scope(
        config.features.managed_machine_updates,
        config.features.runtime_image_origin_enabled,
    );
    let recorder = telemetry::install()?;
    let task_cohorts = telemetry::task_cohort::Cache::default();
    let _metrics_server =
        telemetry::server::Server::start(config.metrics_bind, recorder, task_cohorts.clone())
            .await?;
    #[cfg(feature = "qualification-hooks")]
    let qualification =
        qualification.map(|options| (options, cma_qualification::Registry::default()));
    let composition = observe(
        "composition",
        StartupComposition::build(
            &config,
            #[cfg(feature = "qualification-hooks")]
            qualification.as_ref().map(|(_, registry)| registry.clone()),
        ),
        |_| None,
    )
    .await?;
    let mut composition = match composition {
        StartupComposition::Execution(composition) => *composition,
        StartupComposition::ReleaseQuarantined(router) => {
            let app = with_request_context(with_cors(router, config.cors_origins.clone()));
            let listener = bind(config.bind).await?;
            listening(&config);
            let (http_stop, http_signal) = watch::channel(false);
            let mut server =
                http_server(listener, app, config.http_connection_max_age, http_signal);
            return tokio::select! {
                result = &mut server => http_result(result),
                () = shutdown::shutdown_signal() => drain_http(http_stop, server).await,
            };
        }
    };
    composition.worker_observations = composition
        .worker_observations
        .with_task_audit(task_cohorts, config.task_audit.clone());
    #[cfg(feature = "qualification-hooks")]
    let _qualification_server = if let Some((options, registry)) = qualification {
        Some(
            options
                .bind(
                    registry,
                    composition
                        .agent_manager
                        .as_ref()
                        .map(|manager| manager.coordinator.clone()),
                    &config,
                )
                .await?,
        )
    } else {
        None
    };
    let router = with_cors(
        std::mem::take(&mut composition.router),
        std::mem::take(&mut config.cors_origins),
    );
    let app = with_request_context(image_manager_retirement::mount(router)).layer(axum::Extension(
        telemetry::http::ContextDiagnosticsEnabled(config.features.context_diagnostics),
    ));
    if config.features.am_startup_health_serving {
        return early::serve(&config, composition, app, startup_started).await;
    }
    serve_after_initialization(&config, composition, app, startup_started).await
}

async fn warm_pools(
    composition: &ControlPlaneComposition,
    admission: Option<&Admission>,
    shutdown: &mut ShutdownCoordinator,
) -> Result<(watch::Sender<bool>, WarmPoolStartup), String> {
    let (writer_ready, booting) = watch::channel(false);
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
    if admission.is_some_and(Admission::is_draining) {
        crate::warm_pools::telemetry::startup("stopped");
        return Ok((writer_ready, WarmPoolStartup::Stopping));
    }
    let result = match &composition.runtime_pointer {
        Some(controller) => pointer_pool_startup(controller, admission).await,
        None => fixed_pool_startup(&composition.native_warm_pools, admission).await,
    };
    result.map(|result| (writer_ready, result))
}

pub(crate) async fn pointer_pool_startup(
    controller: &crate::runtime_pointer::RuntimePointerController,
    admission: Option<&Admission>,
) -> Result<WarmPoolStartup, String> {
    observe(
        "default_warm_pool_configuration",
        controller.bootstrap(),
        |_| None,
    )
    .await?;
    observe_pool_readiness(controller.await_active_ready(admission)).await
}

pub(crate) async fn fixed_pool_startup(
    pools: &NativeWarmPools,
    admission: Option<&Admission>,
) -> Result<WarmPoolStartup, String> {
    let held = observe(
        "default_warm_pool_configuration",
        pools.ensure_fixed_defaults(),
        |_| None,
    )
    .await
    .map_err(|error| format!("native warm pool default is not usable: {error:?}"))?;
    observe_pool_readiness(async {
        if admission.is_some_and(Admission::is_draining) {
            return Ok(WarmPoolStartup::Stopping);
        }
        if pools.default_held(false) {
            return Ok(WarmPoolStartup::ConfigurationDrift);
        }
        let readiness = pools
            .await_fixed_default_ready(admission)
            .await
            .map_err(|error| format!("native warm pool startup readiness failed: {error:?}"))?;
        // Capacity deferral carries proof needed by the caller. A held machine
        // lane has already reported its drift during default configuration.
        if held
            && matches!(
                readiness,
                WarmPoolStartup::Ready | WarmPoolStartup::ZeroTarget
            )
        {
            return Ok(WarmPoolStartup::ConfigurationDrift);
        }
        Ok(readiness)
    })
    .await
}

async fn observe_pool_readiness(
    readiness: impl Future<Output = Result<WarmPoolStartup, String>>,
) -> Result<WarmPoolStartup, String> {
    let phase = Phase::new("default_warm_pool_readiness");
    let result = readiness.await;
    let (outcome, desired) = match &result {
        Ok(WarmPoolStartup::Ready) => ("ok", None),
        Ok(WarmPoolStartup::ZeroTarget) => ("zero_target", None),
        Ok(WarmPoolStartup::ConfigurationDrift) => ("configuration_drift", None),
        Ok(WarmPoolStartup::CapacityDeferred(proof)) => {
            ("capacity_deferred", Some(proof.desired() as usize))
        }
        Ok(WarmPoolStartup::Stopping) => ("stopped", None),
        Err(_) => ("error", None),
    };
    crate::warm_pools::telemetry::startup(outcome);
    phase.finish(outcome, desired);
    result
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
    writer_ready: watch::Sender<bool>,
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
    writer_ready.send_replace(true);
    if let Some(controller) = composition.runtime_pointer.clone() {
        shutdown.push(controller.spawn(shutdown.subscribe()));
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
    if let Some(outcomes) = composition.creation_outcomes {
        shutdown.push(outcomes.spawn(shutdown.subscribe()));
    }
    composition.usage.spawn(shutdown);
    composition.usage_metrics.spawn(shutdown);
    if let Some(worker) = composition.document_reclaimer {
        shutdown.push(worker.spawn(shutdown.subscribe()));
    }
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
    let mut shutdown = ShutdownCoordinator::new();
    let (writer_ready, _) = warm_pools(&composition, None, &mut shutdown).await?;
    let listener = bind(config.bind).await?;
    initialize_sandboxes(&composition, None).await?;
    listening(config);
    let manager = composition.agent_manager.clone();
    activate_and_start_workers(composition, &mut shutdown, None, writer_ready).await?;
    serving_entry(startup_started);
    let (http_stop, http_signal) = watch::channel(false);
    let mut server = http_server(listener, app, config.http_connection_max_age, http_signal);
    if config.features.am_failure_containment_enabled {
        let trigger = async {
            use agent_manager::DrainCause;
            match &manager {
                Some(manager) => tokio::select! {
                    _ = shutdown::shutdown_signal() => DrainCause::Shutdown,
                    _ = manager.failed() => DrainCause::OwnershipFailure,
                },
                None => {
                    shutdown::shutdown_signal().await;
                    DrainCause::Shutdown
                }
            }
        };
        return shutdown::serve_contained(server, http_stop, trigger, manager.as_deref(), shutdown)
            .await;
    }
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
