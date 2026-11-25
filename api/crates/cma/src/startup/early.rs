use super::*;

pub(super) async fn serve(
    config: &Config,
    composition: ControlPlaneComposition,
    app: Router,
    startup_started: Instant,
) -> Result<(), String> {
    let listener = bind(config.bind).await?;
    let manager = composition.agent_manager.clone();
    let admission = Admission::new();
    let app = admission.wrap(app, composition.agent_relay.clone());
    let (http_stop, http_signal) = watch::channel(false);
    let mut server = http_server(listener, app, config.http_connection_max_age, http_signal);
    let mut shutdown = ShutdownCoordinator::new();
    let (owner_result, server_exit) = {
        let owner = async {
            let startup = initialize(config, composition, &admission, &mut shutdown);
            let termination = termination(&admission, manager.as_deref());
            let result = until_stopping(&admission, startup, termination, startup_started).await;
            let manager_result = shutdown.drain_agent_manager(manager.as_deref()).await;
            admission.finish_drain(&manager_result);
            result.and(manager_result)
        };
        tokio::pin!(owner);
        tokio::select! {
            biased;
            result = &mut server => {
                admission.close();
                let result = http_result(result)
                    .and(Err("HTTP server exited before shutdown".into()));
                (owner.await, Some(result))
            }
            result = &mut owner => (result, None),
        }
    };
    let server_result = match server_exit {
        Some(result) => result,
        None => drain_http(http_stop, server).await,
    };
    finish(shutdown, owner_result.and(server_result)).await
}

enum Initialization {
    Initialized,
    Stopping,
}

async fn initialize(
    config: &Config,
    composition: ControlPlaneComposition,
    admission: &Admission,
    shutdown: &mut ShutdownCoordinator,
) -> Result<Initialization, String> {
    if admission.is_draining() {
        return Ok(Initialization::Stopping);
    }
    let (writer_ready, outcome) = warm_pools(&composition, Some(admission), shutdown).await?;
    if matches!(outcome, WarmPoolStartup::Stopping) || admission.is_draining() {
        return Ok(Initialization::Stopping);
    }
    initialize_sandboxes(&composition, Some(admission)).await?;
    if admission.is_draining() {
        return Ok(Initialization::Stopping);
    }
    // The phase check admits activation. If draining wins afterward, the
    // owner joins activation and drains any membership it acquired.
    listening(config);
    activate_and_start_workers(composition, shutdown, Some(admission), writer_ready).await?;
    Ok(if admission.is_draining() {
        Initialization::Stopping
    } else {
        Initialization::Initialized
    })
}

async fn until_stopping(
    admission: &Admission,
    startup: impl Future<Output = Result<Initialization, String>>,
    termination: impl Future<Output = Result<(), String>>,
    startup_started: Instant,
) -> Result<(), String> {
    tokio::pin!(startup, termination);
    let result = tokio::select! {
        biased;
        reason = &mut termination => {
            admission.close();
            // Dropping startup here would cancel admitted provider and store work.
            startup.await.map(|_| ()).and(reason)
        }
        result = &mut startup => match result {
            Ok(Initialization::Stopping) => Ok(()),
            Ok(Initialization::Initialized) => tokio::select! {
                biased;
                reason = &mut termination => reason,
                () = std::future::ready(()) => {
                    if admission.open() {
                        serving_entry(startup_started);
                    }
                    termination.await
                }
            },
            Err(error) => Err(error),
        },
    };
    admission.close();
    result
}

async fn termination(
    admission: &Admission,
    manager: Option<&agent_manager::AgentManager>,
) -> Result<(), String> {
    let manager_failure = async {
        if let Some(manager) = manager {
            // Renewal may report a failure before serving. The active manager
            // check also detects clock expiry, and only applies after activation.
            tokio::select! {
                () = signalled(manager.coordinator.failure_signal()) => (),
                () = async {
                    if admission.serving().await {
                        manager.failed().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => (),
            }
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        biased;
        () = shutdown::shutdown_signal() => Ok(()),
        () = manager_failure => Err("Agent Manager lost membership".into()),
        () = admission.stopping() => Ok(()),
    }
}

#[cfg(test)]
#[path = "early_tests.rs"]
mod tests;
