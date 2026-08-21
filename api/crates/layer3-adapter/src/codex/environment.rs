//! What this kernel needs in its environment, and what its sandbox holds.
//!
//! Split from the adapter so both stay within the project's file-size limit,
//! on a division that is real: that file drives a runtime's lifecycle, and this
//! one renders the core's intent into the names this kernel understands. A
//! wrong rendering here grants more than the person asked for, which is why it
//! is worth being able to read on its own — and to inspect without standing up
//! a provider.
//!
//! Everything a deployment configures rather than this kernel decides lives in
//! `hosted::Deployment`. What is left below is this kernel's own: the program
//! the pod runs, where it keeps its conversation, and its private words for how
//! much the agent may do.

use std::collections::BTreeMap;

use crate::hosted::Deployment;
use crate::RuntimeSpec;

/// Where this kernel records its conversation, relative to the pod's home.
///
/// This kernel's own name for its own layout, which is why it is spelled in
/// this kernel's directory. The supervisor is told it at provision; it used to
/// be a constant there, in the one component that is supposed to host any
/// kernel at all.
pub(super) const SESSIONS_DIR: &str = ".codex/sessions";

/// How sandboxes for this kernel are configured.
#[derive(Debug, Clone)]
pub struct CodexSandboxSpec {
    /// What a deployment configures, which is not this kernel's business.
    pub deployment: Deployment,
    /// The subcommand that makes the terminal application continue a named
    /// conversation.
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
}

impl Default for CodexSandboxSpec {
    fn default() -> Self {
        Self {
            deployment: Deployment {
                upstream_model: "gpt-5.6-sol".into(),
                upstream_wire_api: "responses".into(),
                bridge_command: "codex-acp".into(),
                // Verified against the pinned kernel by planting one and
                // asking it: a directory and a SKILL.md is the whole
                // mechanism, and it reports the skill at user scope. No
                // plugin and no marketplace is involved.
                skills_root: ".codex/skills".into(),
                agent_notes: "AGENTS.md".into(),
                ..Deployment::default()
            },
            resume_args: "resume".into(),
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
        let deployment = &self.spec.deployment;
        let mut controlled = deployment.base_environment(spec);
        controlled.extend([
            (
                "CHRONO_RUNTIME_COMMAND".to_string(),
                match conversing {
                    // The bridge, which speaks the contract. Started by the
                    // supervisor with piped stdio rather than under a pty: it
                    // has no screen to paint.
                    true => deployment.bridge_command.clone(),
                    false => "codex".to_string(),
                },
            ),
            (
                // The set the supervisor may be switched between, named at
                // boot. A switch names one of these and is refused otherwise.
                "CHRONO_RUNTIME_COMMANDS".to_string(),
                format!("codex,{}", deployment.bridge_command),
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
            // The file this kernel reads before it starts, composed here and
            // written by the supervisor — which is handed a path and a body and
            // reads neither. It used to be rendered there, in this kernel's
            // vocabulary, into every pod including the ones running something
            // else.
            (
                crate::layer4::CONFIG_PATH_VAR.to_string(),
                super::config::CONFIG_PATH.to_string(),
            ),
            (
                crate::layer4::CONFIG_BODY_VAR.to_string(),
                super::config::render(
                    spec,
                    deployment.proxy_base_url(),
                    &spec
                        .options
                        .model
                        .clone()
                        .unwrap_or_else(|| deployment.upstream_model.clone()),
                    &deployment.upstream_wire_api,
                ),
            ),
            super::config::placeholder(),
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

#[cfg(test)]
#[path = "environment_tests.rs"]
mod environment_tests;
