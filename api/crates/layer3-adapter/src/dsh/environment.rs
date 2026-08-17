//! What this kernel needs in its environment, and what its sandbox holds.
//!
//! Split from the adapter so both stay within the project's file-size limit, on
//! a division that is real: that file drives a runtime's lifecycle, and this one
//! renders the core's intent into the names this kernel understands.
//!
//! Everything a deployment configures rather than this kernel decides lives in
//! `hosted::Deployment`. What is left below is this kernel's own — and it is
//! almost all environment, because the profile baked into the image reads these
//! names at boot. One image therefore serves every deployment: what the adapter
//! puts here decides the route, the model, and how much the agent may do.

use std::collections::BTreeMap;

use crate::hosted::{Deployment, PLACEHOLDER_CREDENTIAL};
use crate::{RuntimePermission, RuntimeSpec};

/// Where this kernel records its conversations, relative to the pod's home.
///
/// The harness's own default under the home the image gives it. Told to the
/// supervisor so it can find one; whether it can be re-served is a separate
/// question, and this kernel answers no — see the adapter's header.
pub(super) const SESSIONS_DIR: &str = ".dsh/sessions";

/// The profile the image bakes, which is the one a sandbox boots.
///
/// Named here rather than left to the launcher's default, because the default
/// profile is the vendor's: telemetry to their endpoint, a route to their API,
/// and a file watcher on the directory the agent is editing.
const PROFILE: &str = "chrono";

/// How sandboxes for this kernel are configured.
#[derive(Debug, Clone)]
pub struct DshSandboxSpec {
    /// What a deployment configures, which is not this kernel's business.
    pub deployment: Deployment,
    /// The route name the baked profile declares, and the one a session asks
    /// the harness for.
    ///
    /// The two must agree or every turn fails with "no adapter registered", so
    /// they are one value rather than two constants in two files.
    pub provider: String,
}

impl Default for DshSandboxSpec {
    fn default() -> Self {
        Self {
            deployment: Deployment {
                upstream_model: "deepseek-v4-flash".into(),
                // What the profile's model route is declared with. The harness
                // borrows another project's protocol names, so this is its own
                // vocabulary rather than the one the first kernel uses.
                upstream_wire_api: "openai-completions".into(),
                bridge_command: format!("dsh --profile {PROFILE}"),
                ..Deployment::default()
            },
            provider: "chrono".into(),
        }
    }
}

impl super::DshAdapter {
    /// The environment a sandbox is started with.
    pub(super) fn environment_for(&self, spec: &RuntimeSpec) -> BTreeMap<String, String> {
        let deployment = &self.spec.deployment;
        let mut controlled = deployment.base_environment(spec);
        controlled.extend([
            (
                "CHRONO_RUNTIME_COMMAND".to_string(),
                deployment.bridge_command.clone(),
            ),
            // Nothing to resume: this kernel cannot continue a carried
            // conversation, and provisioning refuses one outright.
            ("CHRONO_RUNTIME_RESUME_ARGS".to_string(), String::new()),
            (
                "CHRONO_RUNTIME_SNAPSHOT_DIR".to_string(),
                SESSIONS_DIR.to_string(),
            ),
            // Read by the profile the image bakes, which is why these names are
            // this kernel's rather than shared: they configure a model route in
            // a file this adapter never writes.
            (
                "CHRONO_DSH_WIRE_API".to_string(),
                deployment.upstream_wire_api.clone(),
            ),
            (
                "CHRONO_DSH_PROXY_URL".to_string(),
                deployment.proxy_base_url().to_string(),
            ),
            // The inert credential the agent presents to that proxy. The real
            // one is attached by the supervisor and never enters the sandbox.
            (
                "CHRONO_UPSTREAM_PLACEHOLDER_KEY".to_string(),
                PLACEHOLDER_CREDENTIAL.to_string(),
            ),
            // This kernel's own name for how far the agent may reach inside its
            // sandbox. Guarded is absent because it is refused at provision:
            // the harness would stop and ask, and its wire cannot carry the
            // question.
            (
                "DSH_PERMISSION_MODE".to_string(),
                match spec.options.permission {
                    RuntimePermission::Guarded | RuntimePermission::Autonomous => "workspace-write",
                    RuntimePermission::Unrestricted => "danger-full-access",
                }
                .to_string(),
            ),
            // Off, explicitly, rather than relying on the profile's default.
            // A tenant's session activity is not a third party's to receive,
            // and this is the one setting where a silent regression sends it.
            ("DSH_TELEMETRY_MODE".to_string(), "DISABLED".to_string()),
        ]);
        // The session's own variables, added last and refused where they
        // collide, so a caller cannot redirect the agent's traffic or widen
        // what it may reach.
        spec.environment.apply_to(&mut controlled);
        controlled
    }
}

#[cfg(test)]
#[path = "environment_tests.rs"]
mod environment_tests;
