//! What this kernel needs in its environment, and what its sandbox holds.
//!
//! Split from the adapter so both stay within the project's file-size limit,
//! on a division that is real: that file drives a runtime's lifecycle, and this
//! one renders the core's intent into the names this kernel understands.
//!
//! Everything a deployment configures rather than this kernel decides lives in
//! `hosted::Deployment`. What is left below is this kernel's own — and unlike
//! the first kernel, all of it is environment: this one is configured by
//! variables rather than by a file the supervisor writes, so the endpoint it
//! calls and the credential it presents are named here.

use std::collections::BTreeMap;

use crate::hosted::{Deployment, PLACEHOLDER_CREDENTIAL};
use crate::RuntimeSpec;

/// Where this kernel records its conversations, relative to the pod's home.
///
/// Told to the supervisor so it can carry one. It is deliberately the whole
/// tree rather than one conversation's directory: the kernel files each
/// conversation under a directory named for the working directory it was
/// started in, and that name is the kernel's to compute, not ours to predict.
///
/// Carrying is all it buys. Resuming needs the file to land back under the
/// directory the kernel would look in, under the name it would look for, and
/// the supervisor is deliberately naive about both — which is why this kernel
/// declares no snapshot ability. See the adapter's header.
pub(super) const SESSIONS_DIR: &str = ".claude/projects";

/// How sandboxes for this kernel are configured.
#[derive(Debug, Clone)]
pub struct ClaudeCodeSandboxSpec {
    /// What a deployment configures, which is not this kernel's business.
    pub deployment: Deployment,
}

impl Default for ClaudeCodeSandboxSpec {
    fn default() -> Self {
        Self {
            deployment: Deployment {
                upstream_model: "claude-opus-5".into(),
                // The upstream this kernel speaks. Named because the supervisor
                // forwards verbatim and this is what the request will look like.
                upstream_wire_api: "messages".into(),
                bridge_command: "claude-code-acp".into(),
                ..Deployment::default()
            },
        }
    }
}

impl super::ClaudeCodeAdapter {
    /// The environment a sandbox is started with.
    ///
    /// Separated from provisioning because this is where the core's intent is
    /// rendered into this kernel's own vocabulary, and a wrong rendering here
    /// grants more than the person asked for. Worth being able to inspect
    /// without standing up a provider.
    pub(super) fn environment_for(&self, spec: &RuntimeSpec) -> BTreeMap<String, String> {
        let deployment = &self.spec.deployment;
        let mut controlled = deployment.base_environment(spec);
        controlled.extend([
            // Always the bridge. This kernel serves no terminal sessions here,
            // so there is no second program to choose between.
            (
                "CHRONO_RUNTIME_COMMAND".to_string(),
                deployment.bridge_command.clone(),
            ),
            // Nothing to resume: the bridge is told which conversation to
            // continue over the protocol, and this kernel declares it cannot
            // continue a carried one at all.
            ("CHRONO_RUNTIME_RESUME_ARGS".to_string(), String::new()),
            (
                "CHRONO_RUNTIME_SNAPSHOT_DIR".to_string(),
                SESSIONS_DIR.to_string(),
            ),
            // This kernel is configured by environment rather than by a file,
            // so the loopback proxy is named here. The inert credential is what
            // it presents to that proxy; the real one is attached by the
            // supervisor and never enters the sandbox.
            (
                "ANTHROPIC_BASE_URL".to_string(),
                deployment.proxy_base_url().to_string(),
            ),
            (
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                PLACEHOLDER_CREDENTIAL.to_string(),
            ),
            (
                "ANTHROPIC_MODEL".to_string(),
                spec.options
                    .model
                    .clone()
                    .unwrap_or_else(|| deployment.upstream_model.clone()),
            ),
        ]);
        // The session's own variables, added last and refused where they
        // collide. `apply_to` is what enforces that: the names above are how
        // the agent finds its endpoint and what it presents to it, so a caller
        // who could overwrite one could redirect the agent's traffic — and the
        // credential attached to it — somewhere of their choosing.
        spec.environment.apply_to(&mut controlled);
        controlled
    }
}

#[cfg(test)]
#[path = "environment_tests.rs"]
mod environment_tests;
