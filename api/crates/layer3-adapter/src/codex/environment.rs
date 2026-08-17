//! How a sandbox for this kernel is shaped, and what it is started with.
//!
//! Split from the adapter so both stay within the project's file-size limit,
//! on a division that is real: that file drives a runtime's lifecycle, and
//! this one renders the core's intent into the names this kernel understands.
//! A wrong rendering here grants more than the person asked for, which is why
//! it is worth being able to read on its own — and to inspect without standing
//! up a provider.

use std::collections::BTreeMap;

use crate::{RuntimePermission, RuntimeSpec};

/// Where this kernel records its conversation, relative to the pod's home.
///
/// This kernel's own name for its own layout, which is why it is spelled in
/// this kernel's directory. The supervisor is told it at provision; it used to
/// be a constant there, in the one component that is supposed to host any
/// kernel at all.
pub(super) const SESSIONS_DIR: &str = ".codex/sessions";

/// How sandboxes for this kernel are shaped.
#[derive(Debug, Clone)]
pub struct CodexSandboxSpec {
    /// Image carrying the kernel and the supervisor.
    pub image: String,
    /// Entrypoint that starts the supervisor.
    pub entrypoint: Vec<String>,
    /// Hard resource caps for each sandbox.
    pub resource_limits: BTreeMap<String, String>,
    /// Seconds before the provider reclaims the sandbox, or `None` for never.
    ///
    /// `None` is the default and is deliberate. A session is a person's working
    /// terminal: expiring it on a timer ends live work mid-turn, and no timer
    /// can tell an idle session from one whose owner is reading. Teardown is an
    /// explicit act by the owner, and the reconciliation sweep is what reclaims
    /// sandboxes whose session is genuinely gone.
    pub timeout_seconds: Option<u64>,
    /// Where the supervisor forwards the agent's model calls.
    ///
    /// Empty means the agent runs with no model endpoint: still a usable
    /// terminal, but unable to complete a turn.
    pub upstream_base_url: String,
    /// Model the agent should select.
    pub upstream_model: String,
    /// Request shape the upstream speaks.
    pub upstream_wire_api: String,
    /// Root of the identity platform the session's CLI acts against.
    ///
    /// Empty means the session gets a terminal and a model but cannot use the
    /// owner's connected capabilities.
    pub platform_base_url: String,
    /// How this kernel's protocol bridge is invoked.
    ///
    /// A different program from the terminal application above: the same
    /// kernel, driven through a bridge that speaks the contract rather than
    /// painting a screen. Configured rather than fixed for the same reason the
    /// resume verb is — it belongs to the kernel and changes with its
    /// versions, and getting it wrong must be correctable without building a
    /// new image.
    pub bridge_command: String,
    /// The subcommand that makes the agent continue a named conversation.
    ///
    /// The conversation's own identifier is appended to this, so it must be a
    /// form that takes one. Asking for "the latest" instead does not work: the
    /// agent selects among conversations recorded in the current working
    /// directory, and one carried from another pod was recorded somewhere
    /// else, so it finds nothing, starts a fresh conversation and reports
    /// success — silent loss rather than a failure.
    ///
    /// Configured rather than fixed, because this belongs to the agent and
    /// changes between its versions, and getting it wrong has to be
    /// correctable without building a new image.
    pub resume_args: String,
    /// Software the image already carries, as a caller would name it.
    ///
    /// Not a permission list — a statement of fact about the image this
    /// adapter starts. There is no channel for installing anything into a
    /// running sandbox, so a session asking for something outside this is
    /// asking for something it cannot be given, and is refused by name rather
    /// than started without it.
    pub provided_packages: Vec<String>,
    /// Skills the agent in this image can already load, by identifier.
    ///
    /// Same rule and the same reason: no delivery channel exists, so anything
    /// beyond this is a promise the session could not keep.
    pub provided_skills: Vec<String>,
    /// Directory the agent works in, inside its sandbox.
    ///
    /// Told to the bridge when a conversation is opened, because it decides
    /// what the agent can see. The pod's home by default, which is where its
    /// image puts everything a session starts with.
    pub workspace: String,
}

impl Default for CodexSandboxSpec {
    fn default() -> Self {
        Self {
            image: "chrono-bot-runtime:dev".into(),
            entrypoint: vec!["/usr/local/bin/layer4-runtime".into()],
            resource_limits: BTreeMap::from([
                ("cpu".to_string(), "1".to_string()),
                ("memory".to_string(), "1Gi".to_string()),
            ]),
            timeout_seconds: None,
            upstream_base_url: String::new(),
            upstream_model: "gpt-5.6-sol".into(),
            upstream_wire_api: "responses".into(),
            platform_base_url: String::new(),
            resume_args: "resume".into(),
            // Empty by default, which refuses everything. A deployment states
            // what its image carries; guessing here would accept a request the
            // image cannot honour, which is the failure this exists to stop.
            bridge_command: "codex-acp".into(),
            provided_packages: Vec::new(),
            provided_skills: Vec::new(),
            workspace: "/home/chrono".into(),
        }
    }
}

impl super::CodexAdapter {
    /// The environment a sandbox is started with.
    ///
    /// Separated from provisioning because this is where the core's intent is
    /// rendered into this kernel's own vocabulary, and a wrong rendering here
    /// grants more than the person asked for. Worth being able to inspect
    /// without standing up a provider.
    pub(super) fn environment_for(&self, spec: &RuntimeSpec) -> BTreeMap<String, String> {
        let conversing = matches!(spec.options.mode, crate::RuntimeMode::Conversation);
        let mut controlled = BTreeMap::from([
            (
                "CHRONO_RUNTIME_COMMAND".to_string(),
                match conversing {
                    // The bridge, which speaks the contract. Started by the
                    // supervisor with piped stdio rather than under a pty: it
                    // has no screen to paint.
                    true => self.spec.bridge_command.clone(),
                    false => "codex".to_string(),
                },
            ),
            (
                // How the terminal application is told to continue a
                // conversation. The bridge is told by name over the protocol
                // instead, so it needs no verb — and giving it one would put a
                // subcommand on a program that takes none.
                "CHRONO_RUNTIME_RESUME_ARGS".to_string(),
                match conversing {
                    true => String::new(),
                    false => self.spec.resume_args.clone(),
                },
            ),
            (
                // Either way, this kernel has to be told *which* conversation
                // to continue: asked for "the latest" it selects among
                // conversations recorded in the current working directory, and
                // one carried from another pod was recorded somewhere else, so
                // it finds nothing, starts fresh, and reports success.
                "CHRONO_RUNTIME_SNAPSHOT_NAMED".to_string(),
                "true".to_string(),
            ),
            // Where this kernel records its conversation, told to the pod
            // rather than known by it. The supervisor hosts any kernel; the
            // directory belongs to this one, so its name is spelled here.
            (
                "CHRONO_RUNTIME_SNAPSHOT_DIR".to_string(),
                SESSIONS_DIR.to_string(),
            ),
            ("TERM".to_string(), "xterm-256color".to_string()),
            // The supervisor forwards here and attaches the owner's credential.
            // The credential itself never travels as environment: it arrives
            // over the control channel and lives only in supervisor memory.
            (
                "CHRONO_UPSTREAM_BASE_URL".to_string(),
                self.spec.upstream_base_url.clone(),
            ),
            (
                "CHRONO_UPSTREAM_MODEL".to_string(),
                // A model the session asked for wins over the deployment's
                // default; the default is what an unconfigured session gets.
                spec.options
                    .model
                    .clone()
                    .unwrap_or_else(|| self.spec.upstream_model.clone()),
            ),
            // This kernel's own names for the intent the core expressed. The
            // translation lives here because these strings mean nothing to any
            // other runtime.
            (
                "CHRONO_AGENT_EFFORT".to_string(),
                spec.options
                    .reasoning_effort
                    .clone()
                    .unwrap_or_else(|| "medium".to_string()),
            ),
            (
                "CHRONO_AGENT_APPROVAL".to_string(),
                match spec.options.permission {
                    RuntimePermission::Guarded => "on-request",
                    RuntimePermission::Autonomous => "on-failure",
                    // No approvals at all. Survivable only because the sandbox
                    // is still the boundary; this removes the ones inside it.
                    RuntimePermission::Unrestricted => "never",
                }
                .to_string(),
            ),
            (
                "CHRONO_AGENT_SANDBOX".to_string(),
                match spec.options.permission {
                    RuntimePermission::Guarded => "read-only",
                    RuntimePermission::Autonomous => "workspace-write",
                    RuntimePermission::Unrestricted => "danger-full-access",
                }
                .to_string(),
            ),
            (
                "CHRONO_UPSTREAM_WIRE_API".to_string(),
                self.spec.upstream_wire_api.clone(),
            ),
            (
                "CHRONO_PLATFORM_BASE_URL".to_string(),
                self.spec.platform_base_url.clone(),
            ),
            // The CLI reads its endpoint and token from the environment, and
            // environment wins over any saved session. Pointing it at the local
            // proxy with an inert token means the real credential never lands
            // in the sandbox, and the CLI never tries to refresh one itself.
            ("NYXID_URL".to_string(), "http://127.0.0.1:9092".to_string()),
            (
                "NYXID_ACCESS_TOKEN".to_string(),
                "local-proxy-no-credential-needed".to_string(),
            ),
            (
                "CHRONO_RUNTIME_COLS".to_string(),
                spec.size.cols.to_string(),
            ),
            (
                "CHRONO_RUNTIME_ROWS".to_string(),
                spec.size.rows.to_string(),
            ),
        ]);
        // The session's own variables, added last and refused where they
        // collide. `apply_to` is what enforces that: the names above are how
        // the runtime finds its upstream, its proxy and its credential, so a
        // caller who could overwrite one could redirect the agent's traffic —
        // or the credential attached to it — somewhere of their choosing.
        spec.environment.apply_to(&mut controlled);
        controlled
    }
}

/// The first thing asked for that is not on offer, if any.
///
/// Names one rather than all of them: a caller fixes the first and asks again,
/// and a list of everything wrong is longer to read without being more useful.
pub(super) fn absent(wanted: &[String], provided: &[String]) -> Option<String> {
    wanted
        .iter()
        .find(|w| !provided.contains(w))
        .map(|w| w.to_string())
}

#[cfg(test)]
#[path = "environment_tests.rs"]
mod environment_tests;
