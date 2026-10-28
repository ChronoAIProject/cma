//! Kernel-neutral sandbox supervisor for the contract-3 runtime protocol.

mod agent_attachment;
mod agent_filesystem;
mod agent_home;
mod agent_skill_install;
mod bridge;
mod checkpoint;
mod checkpoint_archive;
mod checkpoint_copy;
mod checkpoint_manifest;
mod command;
mod config;
mod control_auth;
mod conversation;
mod desktop_process;
mod failure;
mod fs_durability;
mod git;
mod github_proxy;
mod history;
mod home_initializer;
mod install;
mod install_state;
mod install_state_validation;
mod lanes;
mod link;
mod mutation_authority;
mod mutation_journal;
mod mutation_scope;
mod paths;
mod persistent_home;
mod pool_bootstrap;
mod resource_cgroup;
mod resource_network;
mod resources;
mod restore_bootstrap;
mod skills;
mod snapshot_export;
mod supervisor;
mod tool_proxy;
mod upstream;
mod upstream_compaction;
mod upstream_guard;
mod upstream_target;
mod wire_operation;
mod workspace_journal;
mod workspace_removal_intent;
mod workspace_removal_paths;
mod workspace_removal_writer;
mod workspace_runtime;
mod workspace_setup;
mod workspace_source;
mod worktree;
mod writer_fence;
mod writer_process;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    if let Err(error) = run().await {
        tracing::error!(%error, "runtime supervisor stopped");
        std::process::exit(2);
    }
}

async fn run() -> Result<(), String> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 2
        && (arguments[0] == "--initialize-home" || arguments[0] == "--initialize-home-tree")
    {
        let initialize = if arguments[0] == "--initialize-home-tree" {
            home_initializer::initialize_home_tree
        } else {
            home_initializer::initialize
        };
        return initialize(
            std::path::Path::new(&arguments[1]),
            std::path::Path::new("/usr/share/cma-runtime-home"),
        )
        .map_err(|error| format!("initialize persistent home: {error}"));
    }
    if arguments.len() == 1 && arguments[0] == "--assign-runtime" {
        return pool_bootstrap::assign_from_stdio().await;
    }
    let waiting = arguments.len() == 1 && arguments[0] == "--wait-for-assignment";
    if let Some(command) = arguments.first() {
        if command == "--export-home-snapshot" {
            return snapshot_export::run(&arguments[1..])
                .await
                .map_err(|error| error.to_string());
        }
        if !waiting {
            return Err("unknown runtime command".into());
        }
    }
    let (config, control_auth, _assignment_server) = if waiting {
        let (assigned, server) = pool_bootstrap::wait_for_assignment().await?;
        (assigned.config, Some(assigned.auth), Some(server))
    } else {
        let auth = control_auth::ControlAuth::from_environment()?;
        (config::RuntimeConfig::from_environment()?, auth, None)
    };
    let bind = config.bind;
    let writer_boundary = writer_fence::SupervisorBoundary::install()?;
    let supervisor = supervisor::Supervisor::start(config, writer_boundary)
        .await
        .map_err(|error| error.to_string())?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|error| format!("bind runtime supervisor at {bind}: {error}"))?;
    tracing::info!(%bind, "runtime supervisor listening");
    let mut router = link::router(supervisor);
    if let Some(auth) = control_auth {
        router = router.layer(axum::middleware::from_fn_with_state(
            auth,
            control_auth::authenticate,
        ));
    }
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| format!("serve runtime supervisor: {error}"))
}

async fn shutdown_signal() {
    let interrupt = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "could not install interrupt signal handler");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::warn!(%error, "could not install termination signal handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
}
