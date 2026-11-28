//! Kernel-neutral sandbox supervisor for the contract-3 runtime protocol.

mod agent_attachment;
mod agent_filesystem;
mod agent_home;
mod agent_skill_install;
mod agent_skill_upgrade;
mod bridge;
mod browser;
mod checkpoint;
mod checkpoint_archive;
mod checkpoint_copy;
mod checkpoint_manifest;
mod command;
mod config;
mod control_auth;
mod conversation;
mod desktop_ownership;
mod desktop_process;
mod failure;
mod fs_durability;
mod git;
mod github_proxy;
mod history;
mod home_initializer;
mod home_restore_inspection;
mod install;
mod install_state;
mod install_state_validation;
mod lanes;
mod ledger_format;
mod ledger_import;
mod ledger_reclamation;
mod ledger_storage;
mod link;
mod manager_ownership;
mod mutation_authority;
#[cfg(test)]
#[allow(dead_code)]
#[path = "mutation_authority_v1_tests.rs"]
mod mutation_authority_v1;
mod mutation_capacity;
mod mutation_journal;
mod mutation_ledger;
mod mutation_recovery;
mod mutation_scope;
mod mutation_store;
mod paths;
mod persistent_home;
mod pool_bootstrap;
mod publication_qualification;
mod repository_snapshot;
mod resource_cgroup;
mod resource_network;
mod resources;
mod restore_bootstrap;
mod retirement_archive;
mod retirement_format;
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
mod workspace_prepare_recovery;
mod workspace_removal_intent;
mod workspace_removal_paths;
mod workspace_removal_writer;
mod workspace_runtime;
mod workspace_setup;
mod workspace_source;
mod worktree;
#[cfg(any(target_os = "linux", test))]
mod writer_control_relay;
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
    run_arguments(std::env::args_os().skip(1).collect()).await
}

#[cfg(test)]
thread_local! { static TRAP_SERVING_STARTUP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

async fn run_arguments(arguments: Vec<std::ffi::OsString>) -> Result<(), String> {
    if arguments.len() == 1 && arguments[0] == "--print-build-info" {
        let version = option_env!("CHRONO_BUILD").ok_or("Runtime build version is missing")?;
        let source_sha = option_env!("CHRONO_BUILD_SHA").ok_or("Runtime source SHA is missing")?;
        println!(
            "{}",
            serde_json::json!({"version": version, "source_sha": source_sha})
        );
        return Ok(());
    }
    if arguments
        .first()
        .is_some_and(|arg| arg == "--inspect-restored-home")
    {
        return home_restore_inspection::run(&arguments[1..]);
    }
    #[cfg(target_os = "linux")]
    if arguments.len() == 1 && arguments[0] == desktop_process::guardian::COMMAND {
        return desktop_process::guardian::run().await;
    }
    if arguments
        .first()
        .is_some_and(|arg| arg == "--cutover-ledger-v1")
    {
        return ledger_import::cutover_command()
            .map_err(|error| format!("{}: {error}", error.code));
    }
    if arguments
        .first()
        .is_some_and(|arg| arg == "--export-ledger-v1-evidence")
    {
        if arguments.len() != 2 {
            return Err("ledger_evidence_arguments_invalid".into());
        }
        return ledger_import::evidence::command(arguments[1].clone().into()).await;
    }
    if arguments.len() == 1 && arguments[0] == "--inspect-ledger-v1" {
        return ledger_import::inspect_command().await;
    }
    if arguments.len() == 1 && arguments[0] == "--validate-ledger-v1" {
        return ledger_import::validate_command().await;
    }
    if arguments.len() == 3 && arguments[0] == "--preview-ledger-retirement" {
        let digest = arguments[2]
            .to_str()
            .ok_or("invalid ledger snapshot digest")?;
        return ledger_import::retirement_preview_command(
            arguments[1].clone().into(),
            digest.into(),
        )
        .await;
    }
    if arguments.len() == 2 && arguments[0] == "--plan-ledger-v1" {
        return ledger_import::plan_command(std::path::PathBuf::from(&arguments[1])).await;
    }
    if arguments.len() == 6 && arguments[0] == "--stage-ledger-retirement" {
        let plan_digest = arguments[2].to_str().ok_or("invalid ledger plan digest")?;
        let observation_digest = arguments[4]
            .to_str()
            .ok_or("invalid ledger snapshot digest")?;
        return ledger_import::retirement_stage_command(
            arguments[1].clone().into(),
            plan_digest.into(),
            arguments[3].clone().into(),
            observation_digest.into(),
            arguments[5].clone().into(),
        )
        .await;
    }
    if arguments.len() == 3 && arguments[0] == "--verify-ledger-plan" {
        let digest = arguments[2].to_str().ok_or("invalid ledger plan digest")?;
        return ledger_import::verify_plan_command(arguments[1].clone().into(), digest.into())
            .await;
    }
    if arguments.len() == 3 && arguments[0] == "--verify-ledger-retirement" {
        let digest = arguments[2]
            .to_str()
            .ok_or("invalid ledger retirement digest")?;
        return ledger_import::retirement_verify_command(
            arguments[1].clone().into(),
            digest.into(),
        )
        .await;
    }
    if arguments.len() == 4 && arguments[0] == "--stage-ledger-v1" {
        let digest = arguments[2].to_str().ok_or("invalid ledger plan digest")?;
        return ledger_import::stage_command(
            arguments[1].clone().into(),
            digest.into(),
            arguments[3].clone().into(),
        )
        .await;
    }
    if arguments.len() == 2 && arguments[0] == "--verify-ledger-storage" {
        return ledger_storage::probe(std::path::Path::new(&arguments[1]))
            .await
            .map_err(|error| error.to_string());
    }
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
    let qualification = publication_qualification::requested(&arguments);
    let waiting =
        qualification || (arguments.len() == 1 && arguments[0] == "--wait-for-assignment");
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
    #[cfg(test)]
    if TRAP_SERVING_STARTUP.get() {
        return Err("serving_startup_trapped".into());
    }
    // Native pools invoke this binary directly. Initialize image-specific
    // identity or trust before advertising readiness, never while seeding a reusable home.
    let initializer = std::path::Path::new("/usr/local/libexec/cma/initialize-sandbox");
    if initializer
        .try_exists()
        .map_err(|_| "inspect runtime image initializer")?
    {
        let status = tokio::process::Command::new(initializer)
            .env_clear()
            .status()
            .await
            .map_err(|_| "start runtime image initializer")?;
        if !status.success() {
            return Err("runtime image initialization failed".into());
        }
        tracing::info!("runtime image initialization completed");
    }
    if qualification {
        publication_qualification::arm();
    }
    let (mut config, control_auth, _assignment_server) = if waiting {
        let (assigned, server) = pool_bootstrap::wait_for_assignment().await?;
        if let Some((fault, confirmed)) = assigned.startup_failure {
            pool_bootstrap::apply_staging_startup_failure(&server, fault, confirmed).await?;
        }
        (assigned.config, Some(assigned.auth), Some(server))
    } else {
        let auth = control_auth::ControlAuth::from_environment()?;
        (config::RuntimeConfig::from_environment()?, auth, None)
    };
    config.configure_stale_turn_seal(control_auth.is_some());
    let bind = config.bind;
    let writer_boundary = writer_fence::SupervisorBoundary::install()?;
    let supervisor = supervisor::Supervisor::start(config, writer_boundary)
        .await
        .map_err(|error| error.to_string())?;
    if qualification {
        return Err(
            "publication qualification did not reach the initial publication boundary".into(),
        );
    }
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|error| format!("bind runtime supervisor at {bind}: {error}"))?;
    tracing::info!(
        %bind,
        stale_turn_seal_enabled = supervisor.stale_turn_seal_enabled(),
        "runtime supervisor listening"
    );
    let mut router = link::router(supervisor.clone());
    if let Some(auth) = control_auth {
        router = router.layer(axum::middleware::from_fn_with_state(
            auth,
            control_auth::authenticate,
        ));
    }
    // Runtime WebSockets carry interactive desktop and conversation frames.
    // Flush small frames immediately instead of waiting for TCP coalescing.
    use axum::serve::ListenerExt as _;
    let listener = listener.tap_io(|stream| {
        if let Err(error) = stream.set_nodelay(true) {
            tracing::warn!(%error, "could not disable runtime TCP coalescing");
        }
    });
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            supervisor.shutdown_desktop().await;
        })
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
