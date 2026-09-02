//! CMA sandboxed agent runtime.
//!
//! Runs in the bot pod, one per session. Supervises the agent kernel under a
//! real pseudo-terminal and relays that terminal to the control plane over a
//! WebSocket. This binary is never linked into the control plane.

mod bridge;
mod carry;
mod conversation;
mod conversation_link;
mod gateway_probe;
mod github_capability;
mod lanes;
mod link;
mod prelude;
mod pty;
mod resources;
mod sponsored_model;
mod telemetry;
mod upstream;
mod workspace;
mod workspace_setup;
mod workspace_source;
mod writer_fence;

use std::net::SocketAddr;

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    response::IntoResponse,
    routing::{delete, get, post},
    Router,
};
use ring::rand::SecureRandom as _;
use tokio::sync::{Mutex, RwLock};

use crate::bridge::Bridge;
use crate::github_capability::GithubCapabilityGate;
use crate::pty::PtyRegistry;
use crate::sponsored_model::SponsoredModelGate;
use crate::upstream::{CredentialHolder, UpstreamState};

const MAX_CONVERSATION_SLOTS: usize = 16;

/// One independently supervised conversation process in this pod.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum SlotId {
    /// Compatibility identity for callers that predate named slots.
    Legacy,
    /// Opaque control-plane identity supplied by the attach query.
    Named(String),
}

impl SlotId {
    pub(crate) fn from_query(value: Option<String>) -> Self {
        value.map_or(Self::Legacy, Self::Named)
    }

    #[cfg(test)]
    fn named(value: &str) -> Self {
        Self::Named(value.to_string())
    }
}

#[derive(Default)]
struct ConversationState {
    slots: std::collections::HashMap<SlotId, std::sync::Arc<Bridge>>,
    /// Latches first successful start even if an ended slot is later retired
    /// and its replacement cannot be spawned.
    ever_started: bool,
    /// Process-session roots retained after a bridge primary exits. A bridge
    /// may leave descendants able to mutate the workspace, so replacement
    /// cannot discard their checkpoint authority until procfs proves the
    /// session has no live members.
    retired_writer_roots: std::collections::BTreeSet<i32>,
}

#[derive(Default)]
struct WorkspaceWriterHoldState {
    held: Option<HeldWorkspaceWriters>,
    /// Makes an explicit resume retry idempotent if its first acknowledgement
    /// was lost. A new hold clears this receipt, so older tokens stay stale.
    last_resumed_token: Option<String>,
}

struct HeldWorkspaceWriters {
    epoch: u64,
    version: u64,
    resume_token: String,
    captured: workspace::Captured,
    writers: writer_fence::WriterFence,
    _startup_admission: tokio::sync::OwnedRwLockWriteGuard<()>,
}

impl HeldWorkspaceWriters {
    fn resume(mut self) -> Result<String, String> {
        self.writers.resume()?;
        Ok(self.resume_token)
    }
}

impl ConversationState {
    fn retire_slot(&mut self, slot: &SlotId) {
        if let Some(bridge) = self.slots.remove(slot) {
            if let Some(root) = bridge.writer_session_root() {
                self.retired_writer_roots.insert(root);
            }
        }
    }

    fn has_started(&self) -> bool {
        self.ever_started
    }

    fn writer_session_roots(&mut self) -> Vec<i32> {
        self.retired_writer_roots.retain(|root| {
            match writer_fence::session_has_live_members(*root) {
                Ok(live) => live,
                Err(error) => {
                    // Losing process-table evidence must retain, rather than
                    // silently discard, checkpoint authority.
                    tracing::warn!(%root, %error, "could not retire conversation writer authority");
                    true
                }
            }
        });
        let mut roots = self
            .retired_writer_roots
            .iter()
            .copied()
            .collect::<Vec<_>>();
        roots.extend(
            self.slots
                .values()
                .filter_map(|bridge| bridge.writer_session_root()),
        );
        roots
    }
}

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
    /// Signed GitHub policy shared by the control lane and platform proxy.
    github_capability: GithubCapabilityGate,
    /// Sponsored grant authority shared by the control lane and model proxy.
    /// It is never attached to the separately authorized platform proxy.
    sponsored_model: SponsoredModelGate,
    /// Fixed command-sandbox proof for Trigger runtimes.
    gateway_probe: gateway_probe::GatewayProbe,
    /// This pod's home directory, where the agent keeps its conversation.
    home: std::path::PathBuf,
    /// The repository workspace, kept outside the agent's configuration and
    /// conversation directories.
    workspace_root: std::path::PathBuf,
    /// Sealed checkpoint artifacts live outside the repository tree so they
    /// can never appear in a diff or be recursively captured.
    checkpoint_root: std::path::PathBuf,
    /// Exact Launch workspace authority established by verified restore.
    workspace_authority: std::sync::Arc<RwLock<Option<WorkspaceAuthority>>>,
    /// One capture/version fence at a time within this runtime generation.
    checkpoint_gate: std::sync::Arc<Mutex<()>>,
    /// The one repository clone prepared for conversation worktrees.
    workspace_source: std::sync::Arc<Mutex<workspace_source::WorkspaceSources>>,
    /// Writer and startup-admission authority retained across a successful
    /// hold-for-teardown response.
    workspace_writer_hold: std::sync::Arc<Mutex<WorkspaceWriterHoldState>>,
    /// Serializes pre-start filesystem mutations against the first process
    /// start. Readers may start an agent; writers must finish before any
    /// agent process can observe the filesystem.
    startup_gate: std::sync::Arc<RwLock<()>>,
    /// Linux child-subreaper boundary covering agent processes even after a
    /// double fork, session change, and orphan reparenting.
    writer_supervisor: writer_fence::SupervisorBoundary,
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
    /// Conversations this pod is taking over, keyed by supervisor slot.
    ///
    /// The agent is not started until something attaches, which is what makes
    /// a handover possible over HTTP at all: the conversation can arrive after
    /// the pod is running and still be there before the agent reads it.
    resuming: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<SlotId, String>>>,
    /// What the model endpoint last refused with, if anything.
    ///
    /// Read when a conversation ends badly, so the owner is told the agent
    /// could not reach its model rather than being shown an empty turn.
    pub(crate) model_fault: upstream::UpstreamFault,
    /// The kernel's independently keyed conversation bridges.
    ///
    /// Started lazily, exactly as the pty is, and for the same reason: a pod
    /// runs one process plane, and starting processes nothing is going to talk
    /// to would put kernels on the model endpoint for no reason.
    conversation: std::sync::Arc<Mutex<ConversationState>>,
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
            github_capability: GithubCapabilityGate::disabled(),
            sponsored_model: SponsoredModelGate::new(false, String::new())
                .expect("a disabled sponsored-model gate needs no session"),
            gateway_probe: gateway_probe::GatewayProbe::disabled(std::path::PathBuf::from(
                "/home/chrono/workspace",
            )),
            home: std::path::PathBuf::from("/home/chrono"),
            workspace_root: std::path::PathBuf::from("/home/chrono/workspace"),
            checkpoint_root: std::path::PathBuf::from("/home/chrono/.cma-checkpoints"),
            workspace_authority: std::sync::Arc::new(RwLock::new(None)),
            checkpoint_gate: std::sync::Arc::new(Mutex::new(())),
            workspace_source: std::sync::Arc::new(Mutex::new(
                workspace_source::WorkspaceSources::default(),
            )),
            workspace_writer_hold: std::sync::Arc::new(Mutex::new(
                WorkspaceWriterHoldState::default(),
            )),
            startup_gate: std::sync::Arc::new(RwLock::new(())),
            writer_supervisor: writer_fence::SupervisorBoundary::default(),
            snapshot_dir: None,
            resume_args: None,
            snapshot_named: false,
            resuming: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            // Nothing has been refused, which is the honest state for a pod
            // that has made no upstream call.
            model_fault: upstream::UpstreamFault::default(),
            conversation: std::sync::Arc::new(Mutex::new(ConversationState::default())),
            resources: std::sync::Arc::new(resources::Sampler::new(
                resources::Roots::default(),
                std::time::Instant::now(),
            )),
        }
    }

    /// One keyed conversation bridge, started if it is not already running.
    ///
    /// Lazy for the same reason the pty is: a pod hosts only one kind of
    /// process plane, and which plane is decided by what the control plane
    /// opens. A bridge started eagerly would put a kernel on the model endpoint
    /// for a pod that was only ever going to serve a terminal.
    async fn conversation_bridge(&self, slot: SlotId) -> std::io::Result<std::sync::Arc<Bridge>> {
        let _startup = self.startup_gate.write().await;
        let mut held = self.conversation.lock().await;
        if let Some(running) = held.slots.get(&slot).cloned() {
            // A cached bridge whose process has gone is not a bridge. Handing
            // it back gave a conversation that accepted prompts and answered
            // none — the agent was dead and nothing said so.
            if !running.is_ended() {
                return Ok(running);
            }
            tracing::warn!("the conversation bridge had ended; starting a fresh one");
            held.retire_slot(&slot);
        }
        if self.registry.is_running().await {
            // One session, one plane. A pod driving a terminal and a
            // conversation at once would be two sessions sharing a sandbox,
            // and neither would be the one anybody asked for.
            return Err(std::io::Error::other(
                "this runtime is already hosting a terminal",
            ));
        }
        if held.slots.len() >= MAX_CONVERSATION_SLOTS {
            return Err(std::io::Error::other(
                "this runtime has reached its conversation slot limit",
            ));
        }
        let bridge = std::sync::Arc::new(Bridge::start(&self.agent_command(&slot))?);
        held.ever_started = true;
        held.slots.insert(slot, std::sync::Arc::clone(&bridge));
        Ok(bridge)
    }

    /// Start or reattach the terminal plane without racing another plane or a
    /// pre-start filesystem mutation.
    async fn terminal_session(
        &self,
        id: &str,
        cols: u16,
        rows: u16,
    ) -> std::io::Result<(
        std::sync::Arc<pty::PtySession>,
        tokio::sync::broadcast::Receiver<Vec<u8>>,
    )> {
        let _startup = self.startup_gate.write().await;
        if self.conversation.lock().await.has_started() {
            return Err(std::io::Error::other(
                "this runtime is already hosting a conversation",
            ));
        }
        self.registry
            .get_or_start(
                id,
                &self.agent_command(&SlotId::Legacy),
                cols,
                rows,
                upstream::PLACEHOLDER_KEY,
            )
            .await
    }

    /// The conversation this pod was handed, if it was handed one.
    pub(crate) fn resuming(&self, slot: &SlotId) -> Option<String> {
        match self.resuming.lock() {
            Ok(guard) => guard.get(slot).cloned(),
            Err(poisoned) => poisoned.into_inner().get(slot).cloned(),
        }
    }

    /// The command to start the agent with, given what this pod has been told.
    fn agent_command(&self, slot: &SlotId) -> String {
        let command = match self.command.lock() {
            Ok(held) => held.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let carried = match self.resuming.lock() {
            Ok(guard) => guard.get(slot).cloned(),
            Err(poisoned) => poisoned.into_inner().get(slot).cloned(),
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

    async fn writer_session_roots(&self) -> Vec<i32> {
        let mut roots = self.registry.writer_session_roots().await;
        roots.extend(self.conversation.lock().await.writer_session_roots());
        roots.sort_unstable();
        roots.dedup();
        roots
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceAuthority {
    epoch: u64,
    source_revision: String,
    baseline_manifest_digest: String,
}

#[tokio::main]
async fn main() {
    // The probe helper is the same exact runtime artifact as the supervisor.
    // It must exit before tracing starts so its stdout contains only the
    // closed protocol receipt consumed by its parent.
    if let Some(code) = gateway_probe::run_helper_if_requested().await {
        std::process::exit(code);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let writer_supervisor = match writer_fence::SupervisorBoundary::install() {
        Ok(boundary) => boundary,
        Err(error) => {
            tracing::error!(%error, "workspace-writer supervisor boundary could not be established");
            std::process::exit(2);
        }
    };

    let configured_bind: SocketAddr = std::env::var("CHRONO_RUNTIME_BIND")
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
    let capability_required = match std::env::var("CHRONO_GITHUB_CAPABILITY_REQUIRED")
        .unwrap_or_else(|_| "false".to_string())
        .as_str()
    {
        "true" => true,
        "false" => false,
        _ => {
            tracing::error!("CHRONO_GITHUB_CAPABILITY_REQUIRED must be exactly true or false");
            std::process::exit(2);
        }
    };
    let sponsored_model_required = match std::env::var("CHRONO_SPONSORED_MODEL_REQUIRED")
        .unwrap_or_else(|_| "false".to_string())
        .as_str()
    {
        "true" => true,
        "false" => false,
        _ => {
            tracing::error!("CHRONO_SPONSORED_MODEL_REQUIRED must be exactly true or false");
            std::process::exit(2);
        }
    };
    let isolated_runtime_required = capability_required || sponsored_model_required;
    let discovered_bind_ip = if isolated_runtime_required && configured_bind.ip().is_unspecified() {
        match discover_non_loopback_ip() {
            Ok(ip) => Some(ip),
            Err(error) => {
                tracing::error!(%error, "a Trigger supervisor requires a non-loopback ingress address");
                std::process::exit(2);
            }
        }
    } else {
        None
    };
    let bind = match hardened_runtime_bind(
        configured_bind,
        isolated_runtime_required,
        discovered_bind_ip,
    ) {
        Ok(bind) => bind,
        Err(error) => {
            tracing::error!(%error, "runtime supervisor bind is not safe for Trigger isolation");
            std::process::exit(2);
        }
    };
    if isolated_runtime_required {
        if let Err(error) = start_trigger_loopback_isolation_fronts().await {
            tracing::error!(%error, "Trigger loopback isolation fronts could not start");
            std::process::exit(2);
        }
    }
    let gateway_probe = match gateway_probe::GatewayProbe::from_environment(workspace_root.clone())
    {
        Ok(probe) => probe,
        Err(error) => {
            tracing::error!(%error, "runtime gateway probe configuration is not usable");
            std::process::exit(2);
        }
    };
    if capability_required && !gateway_probe.supported() {
        tracing::error!(
            "a required GitHub capability must also configure the runtime gateway probe"
        );
        std::process::exit(2);
    }
    let session_id = std::env::var("CHRONO_SESSION_ID").unwrap_or_default();
    let sponsored_model =
        match SponsoredModelGate::new(sponsored_model_required, session_id.clone()) {
            Ok(gate) => gate,
            Err(error) => {
                tracing::error!(%error, "sponsored model configuration is not usable");
                std::process::exit(2);
            }
        };
    let reaches = std::env::var("CHRONO_SESSION_REACHES")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let verification_keys =
        std::env::var("CHRONO_GITHUB_CAPABILITY_VERIFY_KEYS").unwrap_or_default();
    let github_capability = match GithubCapabilityGate::from_configuration(
        capability_required,
        session_id,
        reaches,
        &verification_keys,
    ) {
        Ok(gate) => gate,
        Err(error) => {
            tracing::error!(%error, "GitHub capability configuration is not usable");
            std::process::exit(2);
        }
    };

    // The agent talks to this proxy, not to the model endpoint. Configuration
    // is written before anything starts so the first turn already goes through
    // it, and a credential that arrives later needs no restart.
    let proxy_bind: std::net::SocketAddr = std::env::var("CHRONO_UPSTREAM_BIND")
        .unwrap_or_else(|_| "127.0.0.1:9091".to_string())
        .parse()
        .expect("CHRONO_UPSTREAM_BIND must be a socket address");
    if isolated_runtime_required && proxy_bind != "127.0.0.1:9091".parse().unwrap() {
        tracing::error!(%proxy_bind, "a Trigger model proxy must remain on its isolated loopback address");
        std::process::exit(2);
    }
    let upstream_base = std::env::var("CHRONO_UPSTREAM_BASE_URL").unwrap_or_default();
    if !sponsored_model_required {
        if let Err(error) = validate_trigger_upstream(&upstream_base, capability_required) {
            tracing::error!(%error, upstream = "model", "a Trigger runtime upstream is missing");
            std::process::exit(2);
        }
    }

    // What the model endpoint last refused with, if it has refused anything.
    // Held here so the link can report it: the proxy is the only thing that
    // always learns of such a refusal, and an agent's own account of one is
    // unreliable and sometimes absent.
    let mut model_fault = upstream::UpstreamFault::default();

    if upstream_base.trim().is_empty() && !sponsored_model_required {
        // Without an upstream the agent can still run interactively; it simply
        // has nothing to call. Saying so beats a confusing failure mid-turn.
        tracing::warn!(
            "CHRONO_UPSTREAM_BASE_URL is unset; the agent will start but has no model endpoint"
        );
    } else {
        model_fault = match spawn_credential_proxy(
            "model",
            proxy_bind,
            upstream_base.clone(),
            credential.clone(),
            None,
            Some(sponsored_model.clone()),
        )
        .await
        {
            Ok(fault) => fault,
            Err(error) => {
                tracing::error!(%error, "model credential proxy could not start");
                std::process::exit(2);
            }
        };
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
    if let Err(error) = validate_trigger_upstream(&platform_upstream, capability_required) {
        tracing::error!(%error, upstream = "platform", "a Trigger runtime upstream is missing");
        std::process::exit(2);
    }
    if !platform_upstream.trim().is_empty() {
        let platform_bind: std::net::SocketAddr = std::env::var("CHRONO_PLATFORM_PROXY_BIND")
            .unwrap_or_else(|_| gateway_probe::TRIGGER_PLATFORM_PROXY_BIND.to_string())
            .parse()
            .expect("CHRONO_PLATFORM_PROXY_BIND must be a socket address");
        if capability_required
            && platform_bind
                != gateway_probe::TRIGGER_PLATFORM_PROXY_BIND
                    .parse()
                    .expect("the fixed Trigger gateway bind is valid")
        {
            tracing::error!(%platform_bind, "a Trigger platform proxy must use its reserved loopback address");
            std::process::exit(2);
        }
        if let Err(error) = spawn_credential_proxy(
            "platform",
            platform_bind,
            platform_upstream,
            credential.clone(),
            Some(github_capability.clone()),
            None,
        )
        .await
        {
            tracing::error!(%error, "platform credential proxy could not start");
            std::process::exit(2);
        }
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
        github_capability,
        sponsored_model,
        gateway_probe,
        home: home.clone(),
        workspace_root,
        checkpoint_root: home.join(".cma-workspace-checkpoints"),
        workspace_authority: std::sync::Arc::new(RwLock::new(None)),
        checkpoint_gate: std::sync::Arc::new(Mutex::new(())),
        workspace_source: std::sync::Arc::new(Mutex::new(
            workspace_source::WorkspaceSources::default(),
        )),
        workspace_writer_hold: std::sync::Arc::new(Mutex::new(WorkspaceWriterHoldState::default())),
        startup_gate: std::sync::Arc::new(RwLock::new(())),
        writer_supervisor,
        snapshot_dir,
        resume_args,
        snapshot_named,
        resuming: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        conversation: std::sync::Arc::new(Mutex::new(ConversationState::default())),
        resources,
    };

    let app = runtime_router(state);

    let listener = match bind_runtime_supervisor(bind).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(%bind, %error, "runtime supervisor could not start listening");
            std::process::exit(2);
        }
    };
    tracing::info!(%bind, %command, "runtime supervisor listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("runtime supervisor must serve");
}

/// The routes served by every runtime pod.
///
/// Kept in one function so tests exercise the same route-local limits as the
/// listener started by `main`.
fn runtime_router(state: AppState) -> Router {
    Router::new()
        .route("/livez", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .route(
            "/runtime/gateway-probe",
            post(runtime_gateway_probe).layer(DefaultBodyLimit::max(4 * 1024)),
        )
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
        .route(
            "/runtime/prelude",
            post(prelude_write).layer(DefaultBodyLimit::max(prelude::PACK_LIMIT)),
        )
        .route(
            "/runtime/workspace",
            post(workspace_restore).layer(DefaultBodyLimit::max(workspace::MAX_ARCHIVE_BYTES)),
        )
        .route(
            "/runtime/workspace/checkpoint",
            post(workspace_checkpoint).layer(DefaultBodyLimit::max(1)),
        )
        .route(
            "/runtime/workspace/checkpoint/resume",
            post(workspace_checkpoint_resume).layer(DefaultBodyLimit::max(4 * 1024)),
        )
        .route(
            "/runtime/workspace/source",
            post(workspace_source_prepare).layer(DefaultBodyLimit::max(4 * 1024)),
        )
        .route(
            "/runtime/workspace/setup",
            post(workspace_setup_run)
                .layer(DefaultBodyLimit::max(workspace_setup::MAX_REQUEST_BYTES)),
        )
        .route(
            "/runtime/workspace/trees",
            post(workspace_tree_add).layer(DefaultBodyLimit::max(4 * 1024)),
        )
        .route(
            "/runtime/workspace/trees/{name}",
            delete(workspace_tree_remove),
        )
        .with_state(state)
}

fn discover_non_loopback_ip() -> Result<std::net::IpAddr, String> {
    // UDP connect performs only a route lookup; no packet is sent. Inside a
    // sandbox pod it yields the address its provider endpoint reaches, which
    // lets Trigger supervisor ingress avoid every loopback address entirely.
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|error| error.to_string())?;
    socket
        .connect("192.0.2.1:9")
        .map_err(|error| error.to_string())?;
    let ip = socket.local_addr().map_err(|error| error.to_string())?.ip();
    if ip.is_unspecified() || ip.is_loopback() {
        return Err("route lookup did not yield a non-loopback address".into());
    }
    Ok(ip)
}

fn hardened_runtime_bind(
    configured: SocketAddr,
    trigger_required: bool,
    discovered: Option<std::net::IpAddr>,
) -> Result<SocketAddr, &'static str> {
    if !trigger_required {
        return Ok(configured);
    }
    let selected = if configured.ip().is_unspecified() {
        SocketAddr::new(
            discovered.ok_or("a non-loopback runtime ingress address was not discovered")?,
            configured.port(),
        )
    } else {
        configured
    };
    if selected.ip().is_unspecified() || selected.ip().is_loopback() {
        return Err("Trigger supervisor ingress cannot bind wildcard or loopback");
    }
    Ok(selected)
}

fn validate_trigger_upstream(configured: &str, trigger_required: bool) -> Result<(), &'static str> {
    if trigger_required && configured.trim().is_empty() {
        return Err("required Trigger upstream is empty");
    }
    Ok(())
}

async fn start_trigger_loopback_isolation_fronts() -> Result<(), String> {
    let supervisor_bind = gateway_probe::TRIGGER_SUPERVISOR_DENIAL_BIND
        .parse()
        .expect("the fixed supervisor denial bind is valid");
    let model_bind = gateway_probe::TRIGGER_MODEL_PROXY_DENIAL_BIND
        .parse()
        .expect("the fixed model denial bind is valid");
    let [supervisor, model] =
        bind_trigger_loopback_isolation_fronts(supervisor_bind, model_bind).await?;
    for (name, listener) in [("supervisor", supervisor), ("model", model)] {
        tokio::spawn(async move {
            let app = Router::new().fallback(axum::routing::any(trigger_loopback_denial));
            if let Err(error) = axum::serve(listener, app).await {
                tracing::error!(isolation_front = name, %error, "Trigger loopback isolation front stopped");
            }
        });
    }
    Ok(())
}

async fn bind_trigger_loopback_isolation_fronts(
    supervisor_bind: SocketAddr,
    model_bind: SocketAddr,
) -> Result<[tokio::net::TcpListener; 2], String> {
    let supervisor = tokio::net::TcpListener::bind(supervisor_bind)
        .await
        .map_err(|error| error.to_string())?;
    let model = tokio::net::TcpListener::bind(model_bind)
        .await
        .map_err(|error| error.to_string())?;
    Ok([supervisor, model])
}

async fn bind_runtime_supervisor(bind: SocketAddr) -> Result<tokio::net::TcpListener, String> {
    tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|error| error.to_string())
}

async fn trigger_loopback_denial() -> impl IntoResponse {
    (
        axum::http::StatusCode::FORBIDDEN,
        [(
            gateway_probe::ISOLATION_DENIAL_HEADER,
            gateway_probe::ISOLATION_DENIAL_VALUE,
        )],
        "",
    )
}

#[cfg(test)]
mod trigger_isolation_tests {
    use super::*;

    #[test]
    fn trigger_supervisor_bind_requires_a_resolved_non_loopback_ingress() {
        let wildcard: SocketAddr = "0.0.0.0:9090".parse().unwrap();
        assert_eq!(
            hardened_runtime_bind(wildcard, false, None).unwrap(),
            wildcard
        );
        assert_eq!(
            hardened_runtime_bind(wildcard, true, Some("10.42.0.17".parse().unwrap())).unwrap(),
            "10.42.0.17:9090".parse().unwrap()
        );
        assert!(hardened_runtime_bind(wildcard, true, None).is_err());
        assert!(hardened_runtime_bind("127.0.0.1:9090".parse().unwrap(), true, None).is_err());
    }

    #[test]
    fn trigger_requires_both_upstreams_before_readiness() {
        assert!(validate_trigger_upstream("https://service.example", true).is_ok());
        assert!(validate_trigger_upstream("", true).is_err());
        assert!(validate_trigger_upstream("  ", true).is_err());
        assert!(validate_trigger_upstream("", false).is_ok());
    }

    #[tokio::test]
    async fn denial_front_binding_is_atomic_and_ipv4_only() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied_address = occupied.local_addr().unwrap();
        let first_reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let first_address = first_reservation.local_addr().unwrap();
        drop(first_reservation);

        assert!(
            bind_trigger_loopback_isolation_fronts(first_address, occupied_address)
                .await
                .is_err()
        );
        let reclaimed = tokio::net::TcpListener::bind(first_address)
            .await
            .expect("a failed second bind releases the first denial front");
        drop(reclaimed);
        drop(occupied);

        let first = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let first_address = first.local_addr().unwrap();
        let second_address = second.local_addr().unwrap();
        drop(first);
        drop(second);
        let listeners = bind_trigger_loopback_isolation_fronts(first_address, second_address)
            .await
            .unwrap();
        assert!(listeners
            .iter()
            .all(|listener| listener.local_addr().unwrap().is_ipv4()));
        assert_ne!(
            listeners[0].local_addr().unwrap(),
            listeners[1].local_addr().unwrap()
        );
    }

    #[tokio::test]
    async fn isolation_fronts_return_only_the_closed_denial_marker() {
        let response = trigger_loopback_denial().await.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(
            response
                .headers()
                .get(gateway_probe::ISOLATION_DENIAL_HEADER)
                .unwrap(),
            gateway_probe::ISOLATION_DENIAL_VALUE
        );
    }

    #[tokio::test]
    async fn a_credential_proxy_port_collision_fails_before_serving() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = occupied.local_addr().unwrap();
        let result = spawn_credential_proxy(
            "test",
            address,
            "https://upstream.example".into(),
            CredentialHolder::new(),
            None,
            None,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn a_runtime_supervisor_port_collision_is_a_typed_startup_failure() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = occupied.local_addr().unwrap();

        let error = bind_runtime_supervisor(address)
            .await
            .expect_err("an occupied supervisor port must fail before serving");

        assert!(!error.is_empty());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod conversation_writer_tests {
    use super::*;

    #[tokio::test]
    async fn replacing_an_ended_bridge_retains_its_live_descendant_in_the_writer_fence() {
        let state = AppState::for_test(&["/bin/sh"]);
        let first = state
            .conversation_bridge(SlotId::Legacy)
            .await
            .expect("first bridge starts");
        let first_root = first
            .writer_session_root()
            .expect("the first bridge has a process-session root");
        assert!(
            first
                .send(b"trap '' HUP TERM; sleep 3 & exit 0".to_vec())
                .await
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), first.ended())
            .await
            .expect("the first primary exits while its descendant remains");

        let replacement = state
            .conversation_bridge(SlotId::Legacy)
            .await
            .expect("replacement bridge starts");
        let replacement_root = replacement
            .writer_session_root()
            .expect("the replacement bridge has a process-session root");
        assert_ne!(first_root, replacement_root);

        let roots = state.writer_session_roots().await;
        assert!(roots.contains(&first_root));
        assert!(roots.contains(&replacement_root));
        let writer_count = tokio::task::spawn_blocking(move || {
            let mut fence = writer_fence::WriterFence::quiesce(
                roots,
                writer_fence::SupervisorBoundary::default(),
            )?;
            let count = fence.writer_count()?;
            fence.resume()?;
            Ok::<_, String>(count)
        })
        .await
        .expect("the writer-fence task joins")
        .expect("both process sessions quiesce");
        assert!(
            writer_count >= 2,
            "the retired descendant and replacement bridge must both be fenced"
        );

        assert!(replacement.send(b"exit 0".to_vec()).await);
    }

    #[tokio::test]
    async fn every_concurrent_slot_and_its_descendant_is_in_the_writer_fence() {
        let state = AppState::for_test(&["/bin/sh"]);
        let first = state
            .conversation_bridge(SlotId::named("writer-a"))
            .await
            .expect("the first slot starts");
        let second = state
            .conversation_bridge(SlotId::named("writer-b"))
            .await
            .expect("the second slot starts");
        assert!(first.send(b"sleep 10 &".to_vec()).await);
        assert!(second.send(b"sleep 10 &".to_vec()).await);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let roots = state.writer_session_roots().await;
        assert_eq!(roots.len(), 2, "both slot process sessions are authority");
        let writer_count = tokio::task::spawn_blocking(move || {
            let mut fence = writer_fence::WriterFence::quiesce(
                roots,
                writer_fence::SupervisorBoundary::default(),
            )?;
            let count = fence.writer_count()?;
            fence.resume()?;
            Ok::<_, String>(count)
        })
        .await
        .expect("the writer-fence task joins")
        .expect("both slot process trees quiesce");
        assert!(
            writer_count >= 4,
            "both bridges and both descendants must be fenced"
        );

        let cleanup = b"kill $(jobs -p) 2>/dev/null; exit 0".to_vec();
        assert!(first.send(cleanup.clone()).await);
        assert!(second.send(cleanup).await);
    }
}

#[cfg(all(test, unix))]
mod process_plane_tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_terminal_and_conversation_start_exactly_one_plane() {
        let state = AppState::for_test(&["/bin/sh"]);
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));

        let terminal_state = state.clone();
        let terminal_barrier = std::sync::Arc::clone(&barrier);
        let terminal = tokio::spawn(async move {
            terminal_barrier.wait().await;
            terminal_state.terminal_session("race", 80, 24).await
        });

        let conversation_state = state.clone();
        let conversation_barrier = std::sync::Arc::clone(&barrier);
        let conversation = tokio::spawn(async move {
            conversation_barrier.wait().await;
            conversation_state.conversation_bridge(SlotId::Legacy).await
        });

        barrier.wait().await;
        let terminal = terminal.await.expect("the terminal contender joins");
        let conversation = conversation
            .await
            .expect("the conversation contender joins");
        assert_ne!(
            terminal.is_ok(),
            conversation.is_ok(),
            "exactly one process plane must win first startup"
        );

        if terminal.is_ok() {
            state.registry.remove("race").await;
        }
        if let Ok(bridge) = conversation {
            assert!(bridge.send(b"exit 0".to_vec()).await);
        }
    }

    #[tokio::test]
    async fn conversation_slots_reuse_their_bridge_and_enforce_the_advertised_cap() {
        let state = AppState::for_test(&["/bin/cat"]);
        let mut bridges = Vec::new();
        for index in 0..MAX_CONVERSATION_SLOTS {
            bridges.push(
                state
                    .conversation_bridge(SlotId::named(&format!("slot-{index}")))
                    .await
                    .expect("every advertised slot starts"),
            );
        }
        let reused = state
            .conversation_bridge(SlotId::named("slot-0"))
            .await
            .expect("an existing slot reattaches at the cap");
        assert!(std::sync::Arc::ptr_eq(&bridges[0], &reused));

        let overflow = match state
            .conversation_bridge(SlotId::named("one-slot-too-many"))
            .await
        {
            Ok(_) => panic!("a seventeenth bridge must be refused"),
            Err(error) => error,
        };
        assert!(overflow.to_string().contains("slot limit"));
    }

    #[tokio::test]
    async fn a_failed_slot_restart_does_not_reopen_pre_start_mutations() {
        let state = AppState::for_test(&["/usr/bin/true", "/no/such/conversation-bridge"]);
        let ended = state
            .conversation_bridge(SlotId::named("latched"))
            .await
            .expect("the first slot starts");
        tokio::time::timeout(std::time::Duration::from_secs(2), ended.ended())
            .await
            .expect("the first slot ends");
        assert!(state.switch_command("/no/such/conversation-bridge"));
        assert!(
            state
                .conversation_bridge(SlotId::named("latched"))
                .await
                .is_err(),
            "the replacement command is absent"
        );
        assert!(
            agent_started(&state).await,
            "first start remains latched after replacement failure"
        );
    }

    #[tokio::test]
    async fn a_conversation_rejects_a_later_terminal() {
        let state = AppState::for_test(&["/bin/sh"]);
        let conversation = state
            .conversation_bridge(SlotId::Legacy)
            .await
            .expect("the conversation starts");

        let terminal = state.terminal_session("wrong-plane", 80, 24).await;
        assert!(
            matches!(terminal, Err(ref error) if error.to_string().contains("conversation")),
            "the conversation plane must exclude the terminal plane"
        );

        assert!(conversation.send(b"exit 0".to_vec()).await);
    }

    #[tokio::test]
    async fn a_terminal_rejects_a_later_conversation() {
        let state = AppState::for_test(&["/bin/sh"]);
        state
            .terminal_session("terminal-first", 80, 24)
            .await
            .expect("the terminal starts");

        let conversation = state.conversation_bridge(SlotId::Legacy).await;
        assert!(
            matches!(conversation, Err(ref error) if error.to_string().contains("terminal")),
            "the terminal plane must exclude the conversation plane"
        );

        state.registry.remove("terminal-first").await;
    }
}

/// Take a conversation handed over from the runtime this one replaces.
///
/// Refused once the agent is running. Laying a conversation down underneath a
/// live agent would do nothing useful — it reads its history at startup — and
/// accepting the call would report success for a handover that did not happen.
async fn conversation_restore(
    State(state): State<AppState>,
    Query(params): Query<ConversationSnapshotParams>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    if body.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "a conversation to restore was expected",
        )
            .into_response();
    }
    let _startup = state.startup_gate.write().await;
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
    let slot = SlotId::from_query(params.conversation);
    let carried_conversation = conversation::carried_conversation_id(&body);
    match carried_conversation.as_deref() {
        Some(conversation) => {
            let guard = match state.resuming.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(current) = guard.get(&slot) {
                if current != conversation && !matches!(slot, SlotId::Legacy) {
                    tracing::error!(
                        current,
                        attempted = conversation,
                        "a conversation slot was restored with a different identity"
                    );
                    return (
                        axum::http::StatusCode::CONFLICT,
                        "this conversation slot is already bound to a different conversation",
                    )
                        .into_response();
                }
            }
        }
        None if state.snapshot_named || !matches!(slot, SlotId::Legacy) => {
            tracing::error!("the conversation handed over does not name itself");
            return (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                "the conversation does not name itself, so the agent cannot be told to continue it",
            )
                .into_response();
        }
        None => {}
    }
    match conversation::restore(&state.home, sessions_dir, &body, &stamp()) {
        Ok(restored) => {
            match restored.session_id {
                Some(conversation) => {
                    let mut guard = match state.resuming.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    guard.insert(slot, conversation);
                }
                // Whether this matters depends on the kernel, and the adapter
                // that started this pod has already said which it is. One that
                // must be told by name which conversation to continue, and is
                // not told, starts a fresh one and reports success — silent
                // loss, and the runtime this came from is about to be torn
                // down. One that finds its own history was never told anything.
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
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationSnapshotParams {
    #[serde(default)]
    conversation: Option<String>,
}

async fn conversation_snapshot(
    State(state): State<AppState>,
    Query(params): Query<ConversationSnapshotParams>,
) -> impl IntoResponse {
    let Some(sessions_dir) = state.snapshot_dir.as_deref() else {
        return unsupported();
    };
    let captured = match params.conversation.as_deref() {
        Some(conversation_id) => {
            conversation::capture_by_id(&state.home, sessions_dir, conversation_id)
        }
        None => conversation::capture(&state.home, sessions_dir),
    };
    match captured {
        Ok(Some(bytes)) => (
            axum::http::StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        // Nothing to carry is not a failure, and must be distinguishable from
        // one: a caller that read an error as "no conversation" would replace
        // a runtime and silently drop the session's history.
        Ok(None) if params.conversation.is_some() => (
            axum::http::StatusCode::NOT_FOUND,
            "the requested conversation was not found",
        )
            .into_response(),
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
async fn spawn_credential_proxy(
    name: &'static str,
    bind: std::net::SocketAddr,
    upstream_base: String,
    credential: CredentialHolder,
    github_capability: Option<GithubCapabilityGate>,
    sponsored_model: Option<SponsoredModelGate>,
) -> Result<upstream::UpstreamFault, String> {
    let mut state = UpstreamState::new(upstream_base.clone(), credential);
    if let Some(gate) = github_capability {
        state = state.with_github_capability(gate);
    }
    if let Some(gate) = sponsored_model {
        state = state.with_sponsored_model(gate);
    }
    let fault = state.fault();
    let app = Router::new()
        .fallback(axum::routing::any(upstream::proxy))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|error| error.to_string())?;
    let reported = fault.clone();
    tokio::spawn(async move {
        // The configured URL is deliberately not logged. A malformed value may
        // contain userinfo or a query secret before request-time validation
        // refuses it, and the bind plus proxy role is sufficient to diagnose
        // which listener started.
        tracing::info!(proxy = name, %bind, "credential proxy listening");
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(proxy = name, error = %e, "credential proxy stopped");
            // A proxy that stopped is a model endpoint the agent can no longer
            // reach at all, which is worth saying in the same words as a refusal.
            reported.set(format!("the {name} endpoint stopped being reachable: {e}"));
        }
    });
    Ok(fault)
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
    match prelude::write_all(&state.home, &files) {
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
    baseline: String,
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
    if params.baseline.len() != 64 || !params.baseline.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "the workspace baseline digest is invalid",
        )
            .into_response();
    }

    let target = state.workspace_root.clone();
    let archive = body.to_vec();
    let authority = WorkspaceAuthority {
        epoch: params.epoch,
        source_revision: params.source.clone(),
        baseline_manifest_digest: params.baseline.clone(),
    };
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
        Ok(Ok(restored)) => {
            *state.workspace_authority.write().await = Some(authority);
            (
                axum::http::StatusCode::OK,
                axum::Json(serde_json::json!({
                    "workspace_epoch": restored.workspace_epoch,
                    "source_revision": restored.source_revision,
                    "baseline_manifest_digest": params.baseline,
                    "manifest_digest": restored.manifest_digest,
                    "entries": restored.entries,
                    "bytes": restored.bytes,
                })),
            )
                .into_response()
        }
        Ok(Err(_)) => {
            // Workspace errors can contain untrusted repository paths. Keep
            // both logs and the control-plane response on a closed class.
            tracing::warn!("workspace restore refused");
            (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                "workspace restore refused",
            )
                .into_response()
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

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceCheckpointParams {
    epoch: u64,
    version: u64,
    source: String,
    baseline: String,
    #[serde(default)]
    include_git_metadata: bool,
    #[serde(default)]
    disposition: RuntimeWorkspaceCheckpointDisposition,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeWorkspaceCheckpointDisposition {
    #[default]
    ResumeAfterCapture,
    HoldForTeardown,
}

enum WorkspaceCaptureResult {
    Resumed(workspace::Captured),
    Held(workspace::Captured, writer_fence::WriterFence),
}

/// Capture one coherent repository tree while the existing agent remains the
/// session's writer before and after the operation.
async fn workspace_checkpoint(
    State(state): State<AppState>,
    Query(params): Query<WorkspaceCheckpointParams>,
) -> impl IntoResponse {
    let _checkpoint = state.checkpoint_gate.lock().await;
    let authority = state.workspace_authority.read().await.clone();
    if authority.as_ref().is_none_or(|authority| {
        authority.epoch != params.epoch
            || !authority
                .source_revision
                .eq_ignore_ascii_case(&params.source)
            || !authority
                .baseline_manifest_digest
                .eq_ignore_ascii_case(&params.baseline)
    }) {
        return (
            axum::http::StatusCode::CONFLICT,
            "workspace checkpoint authority does not match this runtime",
        )
            .into_response();
    }

    let held_replay = {
        let holds = state.workspace_writer_hold.lock().await;
        match holds.held.as_ref() {
            Some(held)
                if params.disposition == RuntimeWorkspaceCheckpointDisposition::HoldForTeardown
                    && held.epoch == params.epoch
                    && held.version == params.version =>
            {
                Some((held.captured.clone(), held.resume_token.clone()))
            }
            Some(_) => {
                return (
                    axum::http::StatusCode::CONFLICT,
                    "another workspace writer hold is active",
                )
                    .into_response();
            }
            None => None,
        }
    };
    if let Some((captured, token)) = held_replay {
        return checkpoint_held_response(captured, &token);
    }

    let checkpoint_root = state.checkpoint_root.clone();
    let source = params.source.clone();
    let retained = tokio::task::spawn_blocking({
        let checkpoint_root = checkpoint_root.clone();
        let source = source.clone();
        move || {
            workspace::load_capture(
                &checkpoint_root,
                params.epoch,
                params.version,
                &source,
                params.include_git_metadata,
            )
        }
    })
    .await;
    match retained {
        Ok(Ok(Some(captured)))
            if params.disposition == RuntimeWorkspaceCheckpointDisposition::ResumeAfterCapture =>
        {
            return checkpoint_response(captured)
        }
        // A retained hold capture without an in-memory hold receipt must be
        // recaptured under a fresh fence. Returning its bytes alone would lie
        // that writers remained stopped after a process-level interruption.
        Ok(Ok(Some(_))) => {}
        Ok(Ok(None)) => {}
        Ok(Err(_)) => {
            tracing::error!("retained workspace checkpoint is corrupt");
            return (
                axum::http::StatusCode::CONFLICT,
                "retained workspace checkpoint is corrupt",
            )
                .into_response();
        }
        Err(error) => {
            tracing::error!(%error, "workspace checkpoint lookup task failed");
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "workspace checkpoint lookup failed",
            )
                .into_response();
        }
    }

    // Agent input and process starts take the read side. This write guard
    // closes admission before the kernel process groups are stopped.
    let admission = state.startup_gate.clone().write_owned().await;
    let writer_roots = state.writer_session_roots().await;
    let workspace_root = state.workspace_root.clone();
    let writer_supervisor = state.writer_supervisor;
    let disposition = params.disposition;
    let capture = tokio::task::spawn_blocking(move || {
        let mut writers = writer_fence::WriterFence::quiesce(writer_roots, writer_supervisor)?;
        let prepared = workspace::prepare_capture(
            &workspace_root,
            &checkpoint_root,
            params.epoch,
            params.version,
            &source,
            writers.writer_count()?,
            params.include_git_metadata,
        )?;
        match disposition {
            RuntimeWorkspaceCheckpointDisposition::ResumeAfterCapture => {
                writers.resume()?;
                prepared.commit().map(WorkspaceCaptureResult::Resumed)
            }
            RuntimeWorkspaceCheckpointDisposition::HoldForTeardown => {
                let captured = prepared.commit()?;
                Ok(WorkspaceCaptureResult::Held(captured, writers))
            }
        }
    })
    .await;
    match capture {
        Ok(Ok(WorkspaceCaptureResult::Resumed(captured))) => {
            drop(admission);
            checkpoint_response(captured)
        }
        Ok(Ok(WorkspaceCaptureResult::Held(captured, writers))) => {
            let token = match workspace_writer_resume_token() {
                Ok(token) => token,
                Err(error) => {
                    tracing::error!(%error, "workspace writer hold token generation failed");
                    drop(writers);
                    drop(admission);
                    return (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "workspace writer hold could not be established",
                    )
                        .into_response();
                }
            };
            {
                let mut holds = state.workspace_writer_hold.lock().await;
                holds.last_resumed_token = None;
                holds.held = Some(HeldWorkspaceWriters {
                    epoch: params.epoch,
                    version: params.version,
                    resume_token: token.clone(),
                    captured: captured.clone(),
                    writers,
                    _startup_admission: admission,
                });
            }
            checkpoint_held_response(captured, &token)
        }
        Ok(Err(error)) => {
            drop(admission);
            tracing::warn!(
                reason = workspace::checkpoint_refusal_reason(&error),
                "workspace checkpoint refused"
            );
            (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                "workspace checkpoint refused",
            )
                .into_response()
        }
        Err(error) => {
            drop(admission);
            tracing::error!(%error, "workspace checkpoint task failed");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "workspace checkpoint failed",
            )
                .into_response()
        }
    }
}

fn checkpoint_held_response(
    captured: workspace::Captured,
    resume_token: &str,
) -> axum::response::Response {
    match captured.into_wire_held(resume_token) {
        Ok(wire) => (
            axum::http::StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            wire,
        )
            .into_response(),
        Err(_) => {
            tracing::error!("workspace checkpoint hold acknowledgement failed");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "workspace checkpoint hold acknowledgement failed",
            )
                .into_response()
        }
    }
}

fn workspace_writer_resume_token() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "secure random source refused the writer hold token".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceCheckpointResumeRequest {
    resume_token: String,
}

async fn workspace_checkpoint_resume(
    State(state): State<AppState>,
    axum::Json(request): axum::Json<WorkspaceCheckpointResumeRequest>,
) -> impl IntoResponse {
    let _checkpoint = state.checkpoint_gate.lock().await;
    if request.resume_token.len() != 64
        || !request
            .resume_token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "workspace writer resume token is invalid",
        )
            .into_response();
    }
    let held = {
        let mut holds = state.workspace_writer_hold.lock().await;
        if holds.last_resumed_token.as_deref() == Some(request.resume_token.as_str()) {
            return axum::Json(serde_json::json!({
                "resume_token": request.resume_token,
                "writers_resumed": true,
            }))
            .into_response();
        }
        let matches = holds
            .held
            .as_ref()
            .is_some_and(|held| constant_time_token_eq(&held.resume_token, &request.resume_token));
        matches.then(|| holds.held.take()).flatten()
    };
    let Some(held) = held else {
        return (
            axum::http::StatusCode::CONFLICT,
            "workspace writer resume token is stale",
        )
            .into_response();
    };
    match held.resume() {
        Ok(token) => {
            state.workspace_writer_hold.lock().await.last_resumed_token = Some(token.clone());
            axum::Json(serde_json::json!({
                "resume_token": token,
                "writers_resumed": true,
            }))
            .into_response()
        }
        Err(error) => {
            tracing::error!(%error, "workspace writers could not be resumed");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "workspace writers could not be resumed",
            )
                .into_response()
        }
    }
}

fn constant_time_token_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn checkpoint_response(captured: workspace::Captured) -> axum::response::Response {
    match captured.into_wire() {
        Ok(wire) => (
            axum::http::StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            wire,
        )
            .into_response(),
        Err(_) => {
            tracing::error!("workspace checkpoint acknowledgement failed");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "workspace checkpoint acknowledgement failed",
            )
                .into_response()
        }
    }
}

async fn workspace_source_prepare(
    State(state): State<AppState>,
    axum::Json(request): axum::Json<workspace_source::WorkspaceSourceRequest>,
) -> impl IntoResponse {
    if let Err(error) = request.validate() {
        return workspace_source_response(error);
    }

    let _checkpoint = state.checkpoint_gate.lock().await;
    let _startup = state.startup_gate.write().await;
    if agent_started(&state).await {
        return (
            axum::http::StatusCode::CONFLICT,
            "this runtime has already started its agent",
        )
            .into_response();
    }

    let mut sources = state.workspace_source.lock().await;
    match sources.prepare(&state.workspace_root, request).await {
        Ok(receipt) => {
            *state.workspace_authority.write().await = Some(WorkspaceAuthority {
                epoch: receipt.workspace_epoch,
                source_revision: receipt.source_revision.clone(),
                baseline_manifest_digest: receipt.baseline_manifest_digest.clone(),
            });
            axum::Json(receipt).into_response()
        }
        Err(error) => workspace_source_response(error),
    }
}

async fn workspace_setup_run(
    State(state): State<AppState>,
    axum::Json(request): axum::Json<workspace_setup::WorkspaceSetupRequest>,
) -> impl IntoResponse {
    if let Err(detail) = request.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, detail).into_response();
    }

    let _checkpoint = state.checkpoint_gate.lock().await;
    let _startup = state.startup_gate.write().await;
    if agent_started(&state).await {
        return (
            axum::http::StatusCode::CONFLICT,
            "this runtime has already started its agent",
        )
            .into_response();
    }
    let repository = match state
        .workspace_source
        .lock()
        .await
        .setup_directory(&state.workspace_root)
    {
        Ok(repository) => repository,
        Err(error) => return workspace_source_response(error),
    };
    match workspace_setup::run(&repository, &request.commands).await {
        Ok(receipt) => axum::Json(receipt).into_response(),
        Err(error) => {
            tracing::error!(%error, "workspace setup runner failed");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "workspace setup runner failed",
            )
                .into_response()
        }
    }
}

async fn workspace_tree_add(
    State(state): State<AppState>,
    axum::Json(request): axum::Json<workspace_source::WorkspaceTreeRequest>,
) -> impl IntoResponse {
    if let Err(error) = request.validate() {
        return workspace_source_response(error);
    }

    // Checkpoint takes these in the same order. The write side prevents a new
    // process from starting halfway through git's metadata mutation, while an
    // already-running agent remains allowed to request another worktree.
    let _checkpoint = state.checkpoint_gate.lock().await;
    let _startup = state.startup_gate.write().await;
    let mut sources = state.workspace_source.lock().await;
    match sources.add_tree(&state.workspace_root, request).await {
        Ok(receipt) => axum::Json(receipt).into_response(),
        Err(error) => workspace_source_response(error),
    }
}

async fn workspace_tree_remove(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = workspace_source::validate_tree_name(&name) {
        return workspace_source_response(error);
    }

    let _checkpoint = state.checkpoint_gate.lock().await;
    let _startup = state.startup_gate.write().await;
    let mut sources = state.workspace_source.lock().await;
    match sources.remove_tree(&state.workspace_root, &name).await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(error) => workspace_source_response(error),
    }
}

fn workspace_source_response(
    error: workspace_source::WorkspaceSourceError,
) -> axum::response::Response {
    let status = match error {
        workspace_source::WorkspaceSourceError::Invalid(_) => axum::http::StatusCode::BAD_REQUEST,
        workspace_source::WorkspaceSourceError::Conflict(_) => axum::http::StatusCode::CONFLICT,
        workspace_source::WorkspaceSourceError::NotFound => axum::http::StatusCode::NOT_FOUND,
        workspace_source::WorkspaceSourceError::Git { .. } => {
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        }
        workspace_source::WorkspaceSourceError::Timeout(_) => {
            axum::http::StatusCode::GATEWAY_TIMEOUT
        }
        workspace_source::WorkspaceSourceError::Runtime(_) => {
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (status, error.to_string()).into_response()
}

async fn agent_started(state: &AppState) -> bool {
    state.registry.is_running().await || state.conversation.lock().await.has_started()
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let live = state.registry.len().await;
    let reports_resources = state
        .resources
        .sample(std::time::Instant::now())
        .has_observations();
    let github_capability = state.github_capability.snapshot().await;
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
        "prelude": prelude::SUPPORTED,
        "conversationSlots": { "max": MAX_CONVERSATION_SLOTS },
        "workspaceSource": { "version": workspace_source::CAPABILITY_VERSION },
        "workspaceCheckpoint": workspace_checkpoint_readiness(),
        "githubCapability": {
            "name": "github_capability_v2",
            "version": github_capability::POLICY_VERSION,
            "operationClasses": github_capability::GithubOperationClass::ALL
                .iter()
                .map(|operation| operation.wire_name())
                .collect::<Vec<_>>(),
            "runtime": github_capability,
        },
        "runtimeGatewayProbe": {
            "name": gateway_probe::NAME,
            "version": gateway_probe::VERSION,
            "supported": state.gateway_probe.supported(),
        },
        // Declared only when this pod has observed something useful. The route
        // existing proves a wire version, not that this guest exposes any
        // counter. A control plane must still never open the link on a guess:
        // an older pod reads an unfamiliar lane as the terminal.
        "resources": reports_resources,
    }))
}

async fn runtime_gateway_probe(
    State(state): State<AppState>,
    axum::Json(request): axum::Json<gateway_probe::ProbeRequest>,
) -> impl IntoResponse {
    let now_epoch_seconds = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if !state
        .github_capability
        .matches_probe_binding(
            &request.service_id,
            &request.catalog_service_id,
            &request.user_service_slug,
            request.resource_id,
            now_epoch_seconds,
        )
        .await
    {
        return axum::Json(gateway_probe::ProbeReceipt::policy_denied());
    }
    // The helper is another process with workspace visibility. Serialize its
    // entire lifetime with checkpoint admission so Layer 4 cannot start a new
    // potential writer after the fence's final procfs observation.
    let _admission = state.startup_gate.read().await;
    axum::Json(state.gateway_probe.run(request).await)
}

#[cfg(target_os = "linux")]
fn workspace_checkpoint_readiness() -> serde_json::Value {
    serde_json::json!({ "version": workspace::CAPABILITY_VERSION })
}

#[cfg(not(target_os = "linux"))]
fn workspace_checkpoint_readiness() -> serde_json::Value {
    // The production proof relies on Linux procfs process-session enumeration.
    // Development hosts must not claim a capability they cannot enforce.
    serde_json::Value::Null
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

#[cfg(test)]
mod runtime_prelude_router_tests {
    use super::*;
    #[cfg(unix)]
    use std::future::Future;
    #[cfg(unix)]
    use std::pin::Pin;
    #[cfg(unix)]
    use std::task::{Context, Poll};

    const TEST_FILE: &str = "skills/router-boundary/SKILL.md";
    #[cfg(unix)]
    const AXUM_DEFAULT_BODY_LIMIT: usize = 2 * 1024 * 1024;

    struct Pack {
        bytes: Vec<u8>,
        #[cfg(unix)]
        file_len: usize,
    }

    /// A well-formed prelude whose complete HTTP body has exactly `total_len`
    /// bytes. The header contains the file body's length, so solve the small
    /// fixed point instead of assuming a particular JSON representation size.
    fn pack_of_size(total_len: usize) -> Pack {
        let mut file_len = total_len;
        loop {
            let mut header = serde_json::to_vec(&serde_json::json!({
                "chronoPrelude": 1,
                "files": [{ "path": TEST_FILE, "len": file_len }],
            }))
            .expect("the test prelude header serializes");
            let next_file_len = total_len
                .checked_sub(header.len() + 1)
                .expect("the requested pack can hold its header");
            if next_file_len != file_len {
                file_len = next_file_len;
                continue;
            }

            header.push(b'\n');
            header.resize(total_len, b'x');
            return Pack {
                bytes: header,
                #[cfg(unix)]
                file_len,
            };
        }
    }

    struct RuntimeFixture {
        address: std::net::SocketAddr,
        home: std::path::PathBuf,
        #[cfg(unix)]
        state: AppState,
        server: tokio::task::JoinHandle<()>,
    }

    #[cfg(unix)]
    fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
        let waker = futures_util::task::noop_waker();
        future.poll(&mut Context::from_waker(&waker))
    }

    impl RuntimeFixture {
        async fn start(name: &str) -> Self {
            static NEXT_HOME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT_HOME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let home = std::env::temp_dir().join(format!(
                "chrono-runtime-prelude-{}-{sequence}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&home);
            std::fs::create_dir_all(&home).expect("the test runtime home is created");

            let mut state = AppState::for_test(&["/bin/cat"]);
            state.home = home.clone();
            state.snapshot_dir = Some("sessions".into());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("the test runtime listener binds");
            let address = listener
                .local_addr()
                .expect("the test runtime listener has an address");
            let server_state = state.clone();
            let server = tokio::spawn(async move {
                axum::serve(listener, runtime_router(server_state))
                    .await
                    .expect("the test runtime serves");
            });
            Self {
                address,
                home,
                #[cfg(unix)]
                state,
                server,
            }
        }

        async fn post(&self, pack: Vec<u8>) -> reqwest::Response {
            reqwest::Client::new()
                .post(format!("http://{}/runtime/prelude", self.address))
                .body(pack)
                .send()
                .await
                .expect("the test runtime answers the prelude request")
        }

        async fn readiness(&self) -> serde_json::Value {
            let response = reqwest::Client::new()
                .get(format!("http://{}/readyz", self.address))
                .send()
                .await
                .expect("the test runtime answers readiness");
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let body = response
                .bytes()
                .await
                .expect("the readiness body is readable");
            serde_json::from_slice(&body).expect("readiness is JSON")
        }

        fn written_file(&self) -> std::path::PathBuf {
            self.home.join(TEST_FILE)
        }
    }

    impl Drop for RuntimeFixture {
        fn drop(&mut self) {
            self.server.abort();
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_readiness_advertises_prelude_support() {
        let runtime = RuntimeFixture::start("unix-readiness").await;
        assert_eq!(runtime.readiness().await["prelude"], true);
    }

    #[cfg(not(unix))]
    #[tokio::test]
    async fn non_unix_readiness_does_not_advertise_prelude_support() {
        let runtime = RuntimeFixture::start("non-unix-readiness").await;
        assert_eq!(runtime.readiness().await["prelude"], false);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prelude_route_accepts_a_valid_pack_above_axums_default_limit() {
        let runtime = RuntimeFixture::start("above-default").await;
        let pack = pack_of_size(AXUM_DEFAULT_BODY_LIMIT + 1);
        let expected_file_len = pack.file_len;

        let response = runtime.post(pack.bytes).await;

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            std::fs::metadata(runtime.written_file())
                .expect("the accepted prelude was written")
                .len(),
            expected_file_len as u64
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prelude_route_accepts_the_exact_advertised_limit() {
        let runtime = RuntimeFixture::start("exact-limit").await;
        let pack = pack_of_size(prelude::PACK_LIMIT);
        let expected_file_len = pack.file_len;

        let response = runtime.post(pack.bytes).await;

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            std::fs::metadata(runtime.written_file())
                .expect("the exact-limit prelude was written")
                .len(),
            expected_file_len as u64
        );
    }

    #[tokio::test]
    async fn prelude_route_rejects_one_byte_over_the_limit_before_the_handler() {
        let runtime = RuntimeFixture::start("over-limit").await;
        let pack = pack_of_size(prelude::PACK_LIMIT + 1);

        let response = runtime.post(pack.bytes).await;

        // The parser itself would answer 422. A 413 proves the route's body
        // extractor enforced the cap while collecting the request instead.
        assert_eq!(response.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
        assert!(
            !runtime.written_file().exists(),
            "an over-limit body must not reach the prelude writer"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prelude_write_finishes_before_a_queued_terminal_can_start() {
        let runtime = RuntimeFixture::start("terminal-start-race").await;
        let pack = pack_of_size(512);
        let held = runtime.state.startup_gate.write().await;
        let mut write = Box::pin(prelude_write(
            State(runtime.state.clone()),
            axum::body::Bytes::from(pack.bytes),
        ));
        assert!(matches!(poll_once(write.as_mut()), Poll::Pending));
        let mut terminal = Box::pin(runtime.state.terminal_session("race", 80, 24));
        assert!(matches!(poll_once(terminal.as_mut()), Poll::Pending));

        drop(held);
        assert_eq!(
            write.await.into_response().status(),
            axum::http::StatusCode::OK
        );
        assert!(runtime.written_file().exists());
        terminal
            .await
            .expect("the terminal starts only after the prelude is durable");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prelude_write_finishes_before_a_queued_conversation_can_start() {
        let runtime = RuntimeFixture::start("conversation-start-race").await;
        let pack = pack_of_size(512);
        let held = runtime.state.startup_gate.write().await;
        let mut write = Box::pin(prelude_write(
            State(runtime.state.clone()),
            axum::body::Bytes::from(pack.bytes),
        ));
        assert!(matches!(poll_once(write.as_mut()), Poll::Pending));
        let mut conversation = Box::pin(runtime.state.conversation_bridge(SlotId::Legacy));
        assert!(matches!(poll_once(conversation.as_mut()), Poll::Pending));

        drop(held);
        assert_eq!(
            write.await.into_response().status(),
            axum::http::StatusCode::OK
        );
        assert!(runtime.written_file().exists());
        conversation
            .await
            .expect("the conversation starts only after the prelude is durable");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_started_agent_refuses_every_late_pre_start_write() {
        let runtime = RuntimeFixture::start("late-write").await;
        runtime
            .state
            .terminal_session("started", 80, 24)
            .await
            .expect("the terminal starts");

        let response = runtime.post(pack_of_size(512).bytes).await;
        assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
        assert!(!runtime.written_file().exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_live_conversation_refuses_restore_and_a_terminal() {
        let runtime = RuntimeFixture::start("conversation-plane").await;
        runtime
            .state
            .conversation_bridge(SlotId::Legacy)
            .await
            .expect("the conversation starts");

        let carried = carry::seal(
            std::path::Path::new("day/session.jsonl"),
            br#"{"payload":{"id":"conv-started"}}
"#,
        );
        let response = conversation_restore(
            State(runtime.state.clone()),
            Query(ConversationSnapshotParams::default()),
            axum::body::Bytes::from(carried),
        )
        .await
        .into_response();
        assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);

        let terminal = match runtime.state.terminal_session("wrong-plane", 80, 24).await {
            Err(error) => error,
            Ok(_) => panic!("one pod cannot host a terminal beside its conversation"),
        };
        assert!(terminal.to_string().contains("conversation"));
    }
}
