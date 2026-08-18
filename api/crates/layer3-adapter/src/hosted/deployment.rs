//! What a deployment tells a hosted kernel, and what every one of them needs.
//!
//! Kernels differ in the program the pod runs, where that program keeps its
//! conversation, and the names it uses for how much the agent may do. They do
//! not differ in any of this: where the model calls are forwarded, which
//! identity platform the session acts against, how large a sandbox is, or what
//! the image was built carrying.
//!
//! Written down once because it was written twice first. The second kernel is
//! what proved the difference between "what a deployment configures" and "what
//! a kernel is" — the first had them in one struct, where nothing distinguished
//! the fields another kernel would need from the fields only it would.

use std::collections::BTreeMap;

use super::SandboxShape;
use crate::{AdapterError, RuntimeEnvironment, RuntimeSpec};

/// The credential the agent presents to the supervisor's local proxy.
///
/// Inert on purpose, and not a secret: the proxy replaces it outright before
/// anything leaves the pod. It exists only because kernels refuse to start
/// without some credential being present. The real one never enters the
/// sandbox — it arrives over the control channel and lives in supervisor
/// memory, which is what keeps it out of reach of anyone typing in a terminal.
pub const PLACEHOLDER_CREDENTIAL: &str = "local-proxy-no-credential-needed";

/// Where the supervisor serves its credential-attaching proxy, inside the pod.
const PROXY_BASE_URL: &str = "http://127.0.0.1:9091";

/// Where the supervisor serves the identity platform's proxy, inside the pod.
const PLATFORM_PROXY_BASE_URL: &str = "http://127.0.0.1:9092";

/// What a deployment configures about a hosted kernel.
///
/// Every string is empty by default and every list is empty too, which is
/// deliberate in both directions: an unset endpoint points a session nowhere
/// rather than somewhere guessed, and an unstated baseline refuses everything
/// beyond it rather than accepting a request the image cannot honour. A kernel
/// fills in what belongs to it; a deployment fills in the rest.
#[derive(Debug, Clone, Default)]
pub struct Deployment {
    /// How the sandbox itself is shaped.
    pub shape: SandboxShape,
    /// Where the supervisor forwards the agent's model calls.
    ///
    /// Empty means the agent runs with no model endpoint: still a usable
    /// sandbox, but unable to complete a turn.
    pub upstream_base_url: String,
    /// Model the agent should select, when a session names none.
    pub upstream_model: String,
    /// Request shape the upstream speaks.
    pub upstream_wire_api: String,
    /// Root of the identity platform the session's CLI acts against.
    ///
    /// Empty means the session gets a sandbox and a model but cannot use the
    /// owner's connected capabilities.
    pub platform_base_url: String,
    /// How this kernel's protocol bridge is invoked.
    ///
    /// Configured rather than fixed because it belongs to the kernel and
    /// changes with its versions, and getting it wrong has to be correctable
    /// without building a new image.
    pub bridge_command: String,
    /// Software the image already carries, as a caller would name it.
    ///
    /// Not a permission list — a statement of fact about the image this adapter
    /// starts. There is no channel for installing anything into a running
    /// sandbox, so a session asking for something outside this is asking for
    /// something it cannot be given, and is refused by name rather than started
    /// without it.
    pub provided_packages: Vec<String>,
    /// Skills the agent in this image can already load, by identifier.
    ///
    /// Same rule and the same reason: no delivery channel exists, so anything
    /// beyond this is a promise the session could not keep.
    pub provided_skills: Vec<String>,
}

impl Deployment {
    /// The environment every hosted kernel is started with.
    ///
    /// The session's own variables are deliberately *not* applied here. They
    /// are applied last, by the kernel that has finished adding its own names,
    /// because `apply_to` refuses a variable that collides with one already
    /// present — and a name a kernel controls must be protected the same way
    /// as one this file controls.
    pub fn base_environment(&self, spec: &RuntimeSpec) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            // The supervisor forwards here and attaches the owner's credential.
            // The credential itself never travels as environment.
            (
                "CHRONO_UPSTREAM_BASE_URL".to_string(),
                self.upstream_base_url.clone(),
            ),
            (
                "CHRONO_UPSTREAM_MODEL".to_string(),
                // A model the session asked for wins over the deployment's
                // default; the default is what an unconfigured session gets.
                spec.options
                    .model
                    .clone()
                    .unwrap_or_else(|| self.upstream_model.clone()),
            ),
            (
                "CHRONO_UPSTREAM_WIRE_API".to_string(),
                self.upstream_wire_api.clone(),
            ),
            (
                "CHRONO_PLATFORM_BASE_URL".to_string(),
                self.platform_base_url.clone(),
            ),
            // The CLI reads its endpoint and token from the environment, and
            // environment wins over any saved session. Pointing it at the local
            // proxy with an inert token means the real credential never lands
            // in the sandbox, and the CLI never tries to refresh one itself.
            ("NYXID_URL".to_string(), PLATFORM_PROXY_BASE_URL.to_string()),
            (
                "NYXID_ACCESS_TOKEN".to_string(),
                PLACEHOLDER_CREDENTIAL.to_string(),
            ),
            /*
             * What this session may reach, by name, so an agent can find out
             * what it has rather than infer it from a CLI being on PATH.
             *
             * Always set, including when the list is empty: an empty value
             * says "this session reaches nothing", and an absent variable says
             * "nothing told me" — an agent acts differently on those two, so
             * they must not render alike.
             *
             * Here rather than in a kernel's own directory because it is a
             * property of a chrono-bot session, not of any agent: every kernel
             * hosted this way gets it on the same terms.
             */
            ("CHRONO_SESSION_REACHES".to_string(), spec.reaches.join(",")),
            (
                "CHRONO_RUNTIME_COLS".to_string(),
                spec.size.cols.to_string(),
            ),
            (
                "CHRONO_RUNTIME_ROWS".to_string(),
                spec.size.rows.to_string(),
            ),
        ])
    }

    /// Where the agent's model calls go from inside its own sandbox.
    ///
    /// Kernels that are configured by environment rather than by a file are
    /// pointed here, under whatever name they read.
    pub fn proxy_base_url(&self) -> &'static str {
        PROXY_BASE_URL
    }

    /// Refuse anything asked for that this image cannot deliver.
    ///
    /// Variables are delivered. Software and skills are delivered by the image
    /// and the agent in it, and nothing can be added to a running sandbox — so
    /// what the image already carries is honoured and anything else is named
    /// and refused. Accepting it silently would start a session that quietly
    /// lacks the tools it was described as having.
    ///
    /// The same rule for every kernel, because it is a fact about sandboxes
    /// rather than about agents.
    pub fn refuse_undeliverable(
        &self,
        environment: &RuntimeEnvironment,
    ) -> Result<(), AdapterError> {
        if let Some(missing) = absent(&environment.packages, &self.provided_packages) {
            return Err(AdapterError::Unsupported(format!(
                "this runtime's image does not carry {missing}, and software cannot be installed into a running sandbox"
            )));
        }
        if let Some(missing) = absent(&environment.skills, &self.provided_skills) {
            return Err(AdapterError::Unsupported(format!(
                "this runtime's agent cannot load the skill {missing}, and skills cannot be delivered into a running sandbox"
            )));
        }
        Ok(())
    }
}

/// The first thing asked for that is not on offer, if any.
///
/// Names one rather than all of them: a caller fixes the first and asks again,
/// and a list of everything wrong is longer to read without being more useful.
fn absent(wanted: &[String], provided: &[String]) -> Option<String> {
    wanted
        .iter()
        .find(|w| !provided.contains(w))
        .map(|w| w.to_string())
}

#[cfg(test)]
#[path = "deployment_tests.rs"]
mod deployment_tests;
