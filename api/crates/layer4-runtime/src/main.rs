//! CMA sandboxed agent runtime.
//!
//! Runs in the bot pod, one per session. Supervises the agent kernel under a
//! real pseudo-terminal and relays that terminal to the control plane over a
//! WebSocket. This binary is never linked into the control plane.

mod bridge;
mod carry;
mod conversation;
mod conversation_link;
mod lanes;
mod link;
mod prelude;
mod pty;
mod resources;
mod telemetry;
mod upstream;
mod workspace;

use std::net::SocketAddr;

use axum::{
    extract::{DefaultBodyLimit, Query, State},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use tokio::sync::{Mutex, RwLock};

use crate::bridge::Bridge;
use crate::pty::PtyRegistry;
use crate::upstream::{CredentialHolder, UpstreamState};

#[derive(Clone)]
pub(crate) struct AppState {
    registry: PtyRegistry,
    /// The program this pod supervises, swappable in place so a plane change
    /// can restart the program without replacing the pod.
    command: std::sync::Arc<std::sync::Mutex<String>>,
    /// The programs this pod was told at boot it may run. A switch names one
    /// of these and is refused by name otherwise — an arbitrary program is
    /// remote execution.
    commands: Vec<String>,
    /// The owner's credential, swappable while the agent runs.
    credential: CredentialHolder,
    /// This pod's home directory, where the agent keeps its conversation.
    home: std::path::PathBuf,
    /// The repository workspace, kept outside the agent's configuration and
    /// conversation directories.
    workspace_root: std::path::PathBuf,
    /// Serializes pre-start filesystem mutations against the first process
    /// start. Readers may start an agent; writers must finish before any
    /// agent process can observe the filesystem.
    startup_gate: std::sync::Arc<RwLock<()>>,
    /// Where under home this kernel records its conversation, if it records
    /// one at all.
    ///
    /// Told to this pod by the adapter that started it rather than known here.
    /// A supervisor that named one kernel's directory could host only that
    /// kernel — and it is the layer that owns a kernel's name that should be
    /// spelling it.
    ///
    /// `None` means this kernel declared no snapshot support, which is a
    /// different answer from "nothing recorded yet" and is reported as one.
    snapshot_dir: Option<String>,
    /// How this kernel is told to continue a named conversation, if it must be.
    resume_args: Option<String>,
    /// Whether a conversation handed to this runtime has to name itself.
    ///
    /// Stated by the adapter rather than inferred from anything here. A kernel
    /// told by name which conversation to continue, and given no name, starts
    /// a fresh one and reports success — silent loss, discovered by its owner.
    /// Whether that applies is a fact about the kernel, and only the layer
    /// that owns it knows: a kernel resumed by a command verb and one resumed
    /// by a protocol call are both told by name, and neither is distinguishable
    /// from here.
    snapshot_named: bool,
    /// The conversation this pod is taking over, once one has been handed to it.
    ///
    /// The agent is not started until something attaches, which is what makes
    /// a handover possible over HTTP at all: the conversation can arrive after
    /// the pod is running and still be there before the agent reads it.
    resuming: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// What the model endpoint last refused with, if anything.
    ///
    /// Read when a conversation ends badly, so the owner is told the agent
    /// could not reach its model rather than being shown an empty turn.
    pub(crate) model_fault: upstream::UpstreamFault,
    /// The kernel's conversation bridge, once anything has asked for one.
    ///
    /// Started lazily, exactly as the pty is, and for the same reason: a pod
    /// runs one session on one plane, and starting a process nothing is going
    /// to talk to would put a kernel on the model endpoint for no reason.
    conversation: std::sync::Arc<Mutex<Option<std::sync::Arc<Bridge>>>>,
    /// This pod's view of its own consumption.
    ///
    /// One per pod rather than one per link: the cgroup is resolved once, its
    /// one log line is emitted at boot where an operator sees it whether or not
    /// anything ever attaches, and `uptimeMs` counts from when this supervisor
    /// started rather than from when somebody last connected.
    resources: std::sync::Arc<resources::Sampler>,
}

impl AppState {
    /// An otherwise-inert pod that may run these programs.
    ///
    /// For tests that exercise the control lane, which needs the allowlist and
    /// nothing else: no pty is started and no bridge is spawned.
    #[cfg(test)]
    pub(crate) fn for_test(commands: &[&str]) -> Self {
        Self {
            registry: PtyRegistry::new(),
            command: std::sync::Arc::new(std::sync::Mutex::new(
                commands.first().unwrap_or(&"").to_string(),
            )),
            commands: commands.iter().map(|c| (*c).to_string()).collect(),
            credential: CredentialHolder::new(),
            home: std::path::PathBuf::from("/home/chrono"),
            workspace_root: std::path::PathBuf::from("/home/chrono/workspace"),
            startup_gate: std::sync::Arc::new(RwLock::new(())),
            snapshot_dir: None,
            resume_args: None,
            snapshot_named: false,
            resuming: std::sync::Arc::new(std::sync::Mutex::new(None)),
            // Nothing has been refused, which is the honest state for a pod
            // that has made no upstream call.
            model_fault: upstream::UpstreamFault::default(),
            conversation: std::sync::Arc::new(Mutex::new(None)),
            resources: std::sync::Arc::new(resources::Sampler::new(
                resources::Roots::default(),
                std::time::Instant::now(),
            )),
        }
    }

    /// The kernel's conversation bridge, started if it is not already running.
    ///
    /// Lazy for the same reason the pty is: a pod hosts one session on one
    /// plane, and which plane is decided by what the control plane opens. A
    /// bridge started eagerly would be a kernel process on the model endpoint
    /// for a session that was only ever going to be a terminal.
    async fn conversation_bridge(&self) -> std::io::Result<std::sync::Arc<Bridge>> {
        let _startup = self.startup_gate.read().await;
        let mut held = self.conversation.lock().await;
        if let Some(running) = held.as_ref() {
            // A cached bridge whose process has gone is not a bridge. Handing
            // it back gave a conversation that accepted prompts and answered
            // none — the agent was dead and nothing said so.
            if !running.is_ended() {
                return Ok(std::sync::Arc::clone(running));
            }
            tracing::warn!("the conversation bridge had ended; starting a fresh one");
            *held = None;
        }
        if self.registry.is_running().await {
            // One session, one plane. A pod driving a terminal and a
            // conversation at once would be two sessions sharing a sandbox,
            // and neither would be the one anybody asked for.
            return Err(std::io::Error::other(
                "this runtime is already hosting a terminal",
            ));
        }
        let bridge = std::sync::Arc::new(Bridge::start(&self.agent_command())?);
        *held = Some(std::sync::Arc::clone(&bridge));
        Ok(bridge)
    }

    /// The conversation this pod was handed, if it was handed one.
    pub(crate) fn resuming(&self) -> Option<String> {
        match self.resuming.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// The command to start the agent with, given what this pod has been told.
    fn agent_command(&self) -> String {
        let command = match self.command.lock() {
            Ok(held) => held.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let carried = match self.resuming.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let Some(conversation) = carried else {
            return command;
        };
        // Configuration rather than a constant: how a conversation is
        // continued is the agent's decision, it differs between kernels, and
        // it changes between agent versions. Getting it wrong must be
        // correctable without shipping a new binary.
        let Some(resume) = self.resume_args.as_deref() else {
            // This kernel finds its own history; there is nothing to append.
            return command;
        };
        // Named, never "the latest". The agent selects among conversations
        // recorded in the current working directory, and one carried from
        // another pod was recorded somewhere else — so "the latest" finds
        // nothing, starts a fresh conversation, and reports success.
        format!("{command} {resume} {conversation}")
    }

    /// Switch to a different supervised program, if it is one this pod was
    /// told at boot it may run.
    ///
    /// The allowlist is the whole defence: an arbitrary program named here is
    /// remote execution, so anything not handed to this pod at boot is refused
    /// by name rather than started.
    fn switch_command(&self, requested: &str) -> bool {
        if !self.commands.iter().any(|c| c == requested) {
            tracing::warn!(command = %requested, "switch refused: not a command this pod was told to run");
            return false;
        }
        match self.command.lock() {
            Ok(mut held) => {
                *held = requested.to_string();
                tracing::info!(command = %requested, "switching the supervised program");
                true
            }
            Err(_) => false,
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let bind: SocketAddr = std::env::var("CHRONO_RUNTIME_BIND")
        .unwrap_or_else(|_| "0.0.0.0:9090".to_string())
        .parse()
        .expect("CHRONO_RUNTIME_BIND must be a socket address");

    let home: std::path::PathBuf = std::env::var("HOME")
        .unwrap_or_else(|_| "/home/chrono".to_string())
        .into();
    let workspace_root: std::path::PathBuf = std::env::var("CHRONO_RUNTIME_WORKSPACE")
        .unwrap_or_else(|_| "/home/chrono/workspace".to_string())
        .into();

    // Which agent to supervise. Configurable so the same binary can host any
    // kernel, and so a shell can stand in when no kernel is installed.
    let command =
        std::env::var("CHRONO_RUNTIME_COMMAND").unwrap_or_else(|_| "/bin/bash -l".to_string());

    // The programs this pod may be switched to, named at boot. A switch names
    // one of these and is refused otherwise.
    let commands: Vec<String> = std::env::var("CHRONO_RUNTIME_COMMANDS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .collect();

    // Where this kernel records its conversation, and how it is told to
    // continue one. Both are the kernel's business and neither is known here;
    // the adapter that started this pod supplies them. Read once at startup
    // rather than per request, so a pod's behaviour cannot change under it.
    let snapshot_dir = non_empty("CHRONO_RUNTIME_SNAPSHOT_DIR");
    let resume_args = non_empty("CHRONO_RUNTIME_RESUME_ARGS");
    let snapshot_named =
        non_empty("CHRONO_RUNTIME_SNAPSHOT_NAMED").is_some_and(|v| v.eq_ignore_ascii_case("true"));
    match snapshot_dir.as_deref() {
        Some(dir) => tracing::info!(dir, "this runtime can hand its conversation over"),
        // Said at startup rather than discovered at rotation, when the answer
        // decides whether a session can be carried anywhere.
        None => tracing::info!(
            "no snapshot directory configured; this runtime cannot hand its conversation over"
        ),
    }

    let credential = CredentialHolder::new();

    // The agent talks to this proxy, not to the model endpoint. Configuration
    // is written before anything starts so the first turn already goes through
    // it, and a credential that arrives later needs no restart.
    let proxy_bind: std::net::SocketAddr = std::env::var("CHRONO_UPSTREAM_BIND")
        .unwrap_or_else(|_| "127.0.0.1:9091".to_string())
        .parse()
        .expect("CHRONO_UPSTREAM_BIND must be a socket address");
    let upstream_base = std::env::var("CHRONO_UPSTREAM_BASE_URL").unwrap_or_default();

    // What the model endpoint last refused with, if it has refused anything.
    // Held here so the link can report it: the proxy is the only thing that
    // always learns of such a refusal, and an agent's own account of one is
    // unreliable and sometimes absent.
    let mut model_fault = upstream::UpstreamFault::default();

    if upstream_base.trim().is_empty() {
        // Without an upstream the agent can still run interactively; it simply
        // has nothing to call. Saying so beats a confusing failure mid-turn.
        tracing::warn!(
            "CHRONO_UPSTREAM_BASE_URL is unset; the agent will start but has no model endpoint"
        );
    } else {
        model_fault = spawn_credential_proxy(
            "model",
            proxy_bind,
            upstream_base.clone(),
            credential.clone(),
        );
    }

    // Whatever this kernel needs on disk before it starts, written where it
    // was told and never read. Which kernel this is, what it needs and where
    // it goes are the adapter's to say — a supervisor that knew any of them
    // would be a supervisor that can host one kernel.
    match prelude::write(&home) {
        Ok(Some(_)) => {}
        Ok(None) => tracing::info!("this kernel asked for no configuration file"),
        Err(e) => {
            // Loud rather than warned about. A kernel started without the
            // configuration it was meant to have runs differently from the
            // session that was ordered, and does it silently.
            tracing::error!(error = %e, "cannot write the kernel configuration");
            std::process::exit(2);
        }
    }

    // The agent also drives a CLI that acts on the owner's behalf against the
    // identity platform itself. It gets its own listener onto the platform root,
    // sharing one credential so a single renewal updates both.
    let platform_upstream = std::env::var("CHRONO_PLATFORM_BASE_URL").unwrap_or_default();
    if !platform_upstream.trim().is_empty() {
        let platform_bind: std::net::SocketAddr = std::env::var("CHRONO_PLATFORM_PROXY_BIND")
            .unwrap_or_else(|_| "127.0.0.1:9092".to_string())
            .parse()
            .expect("CHRONO_PLATFORM_PROXY_BIND must be a socket address");
        spawn_credential_proxy(
            "platform",
            platform_bind,
            platform_upstream,
            credential.clone(),
        );
    }

    let resources = std::sync::Arc::new(resources::Sampler::new(
        resources::Roots::default(),
        std::time::Instant::now(),
    ));
    let state = AppState {
        registry: PtyRegistry::new(),
        model_fault: model_fault.clone(),
        command: std::sync::Arc::new(std::sync::Mutex::new(command.clone())),
        commands,
        credential,
        home: home.clone(),
        workspace_root,
        startup_gate: std::sync::Arc::new(RwLock::new(())),
        snapshot_dir,
        resume_args,
        snapshot_named,
        resuming: std::sync::Arc::new(std::sync::Mutex::new(None)),
        conversation: std::sync::Arc::new(Mutex::new(None)),
        resources,
    };

    let app = Router::new()
        .route("/livez", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .route("/runtime/{id}/terminal", get(link::attach))
        // What makes this pod replaceable. The control plane takes this before
        // tearing the pod down and hands it to the one that takes over.
        .route(
            "/runtime/conversation",
            get(conversation_snapshot).post(conversation_restore),
        )
        // Files a kernel needs before it reads anything. Taken here rather
        // than through the environment because what they carry does not fit
        // there: one skill alone is larger than a single environment string
        // may be, and the pod would fail to start with nothing naming why.
        .route("/runtime/prelude", post(prelude_write))
        .route(
            "/runtime/workspace",
            post(workspace_restore).layer(DefaultBodyLimit::max(workspace::MAX_ARCHIVE_BYTES)),
        )
        .with_state(state);

    tracing::info!(%bind, %command, "runtime supervisor listening");
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .expect("runtime supervisor must bind");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("runtime supervisor must serve");
}

/// Take a conversation handed over from the runtime this one replaces.
///
/// Refused once the agent is running. Laying a conversation down underneath a
/// live agent would do nothing useful — it reads its history at startup — and
/// accepting the call would report success for a handover that did not happen.
async fn conversation_restore(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let _startup = state.startup_gate.write().await;
    if body.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "a conversation to restore was expected",
        )
            .into_response();
    }
    if agent_started(&state).await {
        return (
            axum::http::StatusCode::CONFLICT,
            "this runtime has already started its agent",
        )
            .into_response();
    }
    let Some(sessions_dir) = state.snapshot_dir.as_deref() else {
        return unsupported();
    };
    match conversation::restore(&state.home, sessions_dir, &body, &stamp()) {
        Ok(restored) => {
            match restored.session_id {
                Some(conversation) => match state.resuming.lock() {
                    Ok(mut guard) => *guard = Some(conversation),
                    Err(poisoned) => *poisoned.into_inner() = Some(conversation),
                },
                // Whether this matters depends on the kernel, and the adapter
                // that started this pod has already said which it is. One that
                // must be told by name which conversation to continue, and is
                // not told, starts a fresh one and reports success — silent
                // loss, and the runtime this came from is about to be torn
                // down. One that finds its own history was never told anything.
                None if state.snapshot_named => {
                    tracing::error!("the conversation handed over does not name itself");
                    return (
                        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                        "the conversation does not name itself, so the agent cannot be told to continue it",
                    )
                        .into_response();
                }
                None => tracing::info!(
                    "the conversation handed over names no identifier; this kernel finds its own"
                ),
            }
            axum::http::StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            // Loud, because the caller is about to tear down the runtime this
            // conversation came from. Reporting success here would lose it.
            tracing::error!(error = %e, "cannot restore the conversation handed to this runtime");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot restore the conversation: {e}"),
            )
                .into_response()
        }
    }
}

/// An environment value, treating blank as absent.
///
/// A key set to the empty string is how a deployment says "not this one" in a
/// manifest, and reading that as a configured value would point a snapshot at
/// the home directory itself.
fn non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// A filename-safe stamp, distinct per restore within a pod.
fn stamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    format!("carried-{seconds}")
}

/// This runtime declared no snapshot support.
///
/// Distinct from having nothing recorded yet, and deliberately so: a caller
/// that read "cannot" as "nothing" would replace a runtime and silently drop
/// the session's history.
fn unsupported() -> axum::response::Response {
    (
        axum::http::StatusCode::NOT_IMPLEMENTED,
        "this runtime does not capture its agent's conversation",
    )
        .into_response()
}

/// Hand over the agent's conversation so it can be resumed elsewhere.
async fn conversation_snapshot(State(state): State<AppState>) -> impl IntoResponse {
    let Some(sessions_dir) = state.snapshot_dir.as_deref() else {
        return unsupported();
    };
    match conversation::capture(&state.home, sessions_dir) {
        Ok(Some(bytes)) => (
            axum::http::StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        // Nothing to carry is not a failure, and must be distinguishable from
        // one: a caller that read an error as "no conversation" would replace
        // a runtime and silently drop the session's history.
        Ok(None) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "cannot capture the conversation");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot capture the conversation: {e}"),
            )
                .into_response()
        }
    }
}

/// Start a listener that forwards to `upstream` with the owner's credential.
///
/// Several of these run at once, one per upstream the agent talks to, all
/// sharing a single credential holder so one renewal reaches every consumer.
fn spawn_credential_proxy(
    name: &'static str,
    bind: std::net::SocketAddr,
    upstream_base: String,
    credential: CredentialHolder,
) -> upstream::UpstreamFault {
    let state = UpstreamState::new(upstream_base.clone(), credential);
    let fault = state.fault();
    let app = Router::new()
        .fallback(axum::routing::any(upstream::proxy))
        .with_state(state);
    let reported = fault.clone();
    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(bind).await {
            Ok(listener) => {
                tracing::info!(proxy = name, %bind, upstream = %upstream_base, "credential proxy listening");
                if let Err(e) = axum::serve(listener, app).await {
                    tracing::error!(proxy = name, error = %e, "credential proxy stopped");
                    // A proxy that stopped is a model endpoint the agent can
                    // no longer reach at all, which is worth saying in the
                    // same words as a refusal.
                    reported.set(format!("the {name} endpoint stopped being reachable: {e}"));
                }
            }
            Err(e) => {
                tracing::error!(proxy = name, %bind, error = %e, "credential proxy cannot bind");
                reported.set(format!("the {name} endpoint could not be started: {e}"));
            }
        }
    });
    fault
}

/// Take files a kernel needs and put them on disk.
///
/// Refused once the agent has started: it reads what it was given at startup,
/// so a write landing afterwards is a file the session was promised and is not
/// using. Saying so is better than writing it and letting somebody believe it
/// took effect.
///
/// The supervisor reads none of this. It is told paths and bodies and writes
/// them, exactly as it is told what to run and where a conversation lives —
/// a kernel that changes what it needs must not be able to break the pod.
async fn prelude_write(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let _startup = state.startup_gate.write().await;
    if agent_started(&state).await {
        return (
            axum::http::StatusCode::CONFLICT,
            "this runtime has already started its agent",
        )
            .into_response();
    }
    let files = match prelude::unpack(&body) {
        Ok(files) => files,
        Err(why) => return (axum::http::StatusCode::UNPROCESSABLE_ENTITY, why).into_response(),
    };
    let home = std::path::PathBuf::from(
        std::env::var("HOME").unwrap_or_else(|_| "/home/chrono".to_string()),
    );
    match prelude::write_all(&home, &files) {
        Ok(written) => (axum::http::StatusCode::OK, format!("{written}")).into_response(),
        Err(e) => {
            // Loud: the session was told it would have these.
            tracing::error!(error = %e, "cannot write the prelude");
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceRestoreParams {
    epoch: u64,
    source: String,
    manifest: String,
    archive: String,
}

/// Restore one complete repository tree before this runtime starts its agent.
///
/// Metadata is repeated outside the archive so neither side can acknowledge a
/// different object than the control plane selected. The archive is verified
/// and materialized in a staging directory before its final path is visible.
async fn workspace_restore(
    State(state): State<AppState>,
    Query(params): Query<WorkspaceRestoreParams>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let _startup = state.startup_gate.write().await;
    if agent_started(&state).await {
        return (
            axum::http::StatusCode::CONFLICT,
            "this runtime has already started its agent",
        )
            .into_response();
    }
    if body.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "a workspace archive was expected",
        )
            .into_response();
    }

    let target = state.workspace_root.clone();
    let archive = body.to_vec();
    let restored = tokio::task::spawn_blocking(move || {
        workspace::restore(
            &target,
            &archive,
            params.epoch,
            &params.source,
            &params.manifest,
            &params.archive,
        )
    })
    .await;
    match restored {
        Ok(Ok(restored)) => (axum::http::StatusCode::OK, axum::Json(restored)).into_response(),
        Ok(Err(why)) => {
            tracing::warn!(reason = %why, "workspace restore refused");
            (axum::http::StatusCode::UNPROCESSABLE_ENTITY, why).into_response()
        }
        Err(error) => {
            tracing::error!(%error, "workspace restore task failed");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "workspace restore failed",
            )
                .into_response()
        }
    }
}

async fn agent_started(state: &AppState) -> bool {
    state.registry.is_running().await || state.conversation.lock().await.is_some()
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let live = state.registry.len().await;
    let reports_resources = state
        .resources
        .sample(std::time::Instant::now())
        .has_observations();
    // Whether a credential is held, never which one: an operator needs to know
    // if calls would fail, not what the value is.
    axum::Json(serde_json::json!({
        "status": "ready",
        "runtimes": live,
        "credential": state.credential.is_set().await,
        // Declared so a control plane can tell an image that takes a prelude
        // from one that does not. §6 makes the runtime image a separate
        // deliberate bump, so the two versions genuinely coexist, and a POST
        // to a router without the route answers 404 — which the adapter reads
        // as a sandbox that is gone and replaces a healthy one.
        "prelude": true,
        "workspaceCheckpoint": { "version": workspace::VERSION },
        // Declared only when this pod has observed something useful. The route
        // existing proves a wire version, not that this guest exposes any
        // counter. A control plane must still never open the link on a guess:
        // an older pod reads an unfamiliar lane as the terminal.
        "resources": reports_resources,
    }))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("ctrl-c handler installs");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler installs")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
