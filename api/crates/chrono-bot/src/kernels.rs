//! Which kernels this deployment runs, and what it configures them with.
//!
//! The composition root's one job that is worth its own file: this is the only
//! place in the service where a concrete kernel is named. Everything upstream
//! selects by opaque identifier, which is what lets a kernel arrive as a
//! directory in Layer 3 and a block here rather than as a change to anything
//! that uses one.

use std::collections::HashMap;
use std::sync::Arc;

use layer2_core::KernelId;
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    hosted::Deployment,
    sandbox::SandboxClient,
    KernelAdapter,
};

use crate::config::Config;

/// Every kernel this deployment offers, by the identifier a session names.
pub fn registry(config: &Config) -> HashMap<KernelId, Arc<dyn KernelAdapter>> {
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    // The only place a concrete runtime implementation is named. Everything
    // upstream of here selects by opaque identifier, which is what lets a
    // kernel arrive as a directory in Layer 3 and a block here.
    let mut codex = CodexSandboxSpec::default();
    deploy(config, &mut codex.deployment);
    codex.deployment.upstream_base_url = config.upstream_base_url.clone();
    codex.deployment.upstream_model = config.upstream_model.clone();
    codex.deployment.upstream_wire_api = config.upstream_wire_api.clone();
    adapters.insert(
        KernelId::new("codex"),
        Arc::new(CodexAdapter::new(
            SandboxClient::new(
                config.sandbox_base_url.clone(),
                config.sandbox_api_key.clone(),
            ),
            codex,
        )),
    );

    for (id, adapter) in adapters.iter() {
        let c = adapter.capabilities();
        tracing::info!(
            kernel = %id,
            conversation = c.conversation,
            terminal = c.terminal,
            snapshot = c.snapshot,
            steer = c.steer,
            usage = c.usage,
            "kernel registered"
        );
    }
    adapters
}

/// Apply what this deployment configures onto a kernel's own defaults.
///
/// The sandbox's size and image, the identity platform, and what the image was
/// built carrying are facts about this deployment rather than about an agent,
/// so they are stated once and every kernel gets the same ones.
///
/// Written as an overlay rather than as a constructor on purpose. A
/// constructor here would return a whole `Deployment`, and every field it did
/// not know about — the program a pod runs, the request shape its kernel
/// speaks — would come back empty and silently replace what that kernel had
/// set for itself.
fn deploy(config: &Config, deployment: &mut Deployment) {
    deployment.shape.image = config.sandbox_image.clone();
    deployment.shape.resource_limits = std::collections::BTreeMap::from([
        ("cpu".to_string(), config.sandbox_cpu.clone()),
        ("memory".to_string(), config.sandbox_memory.clone()),
    ]);
    deployment.platform_base_url = config.platform_base_url.clone();
    // What the image is built with, which is the same list this deployment
    // promises every session. Stated to the adapter so a profile asking for
    // anything else is refused at provision rather than started without it.
    deployment.provided_packages = config.baseline_packages.clone();
    deployment.provided_skills = config.baseline_skills.clone();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deployment's configuration, as `Config::from_env` would resolve it.
    fn deployment_config() -> Config {
        let mut config = Config::sample();
        config.sandbox_image = "ghcr.io/example/runtime:1".into();
        config.sandbox_cpu = "2".into();
        config.sandbox_memory = "4Gi".into();
        config.baseline_packages = vec!["nyxid".to_string()];
        config
    }

    #[test]
    fn a_deployment_never_overwrites_what_a_kernel_set_for_itself() {
        // The defect this guards: a deployment overlay written as a constructor
        // returns every field, including the ones it knows nothing about, and
        // the program a pod runs comes back empty. The session then starts a
        // sandbox with no command in it.
        let kernels = [CodexSandboxSpec::default().deployment];
        for mut deployment in kernels {
            let bridge = deployment.bridge_command.clone();
            let wire = deployment.upstream_wire_api.clone();
            assert!(
                !bridge.is_empty() && !wire.is_empty(),
                "a kernel names both"
            );

            deploy(&deployment_config(), &mut deployment);

            assert_eq!(deployment.bridge_command, bridge);
            assert_eq!(deployment.upstream_wire_api, wire);
        }
    }

    #[test]
    fn every_kernel_registered_is_reachable_by_the_identifier_it_names_itself() {
        // A session selects a kernel by identifier, and nothing between the
        // request and here checks that the two agree. They must, or a session
        // asking for a kernel this deployment runs is told there is no such
        // kernel.
        for (id, adapter) in registry(&Config::sample()) {
            assert_eq!(id.0, adapter.kernel_name(), "registered under the wrong id");
        }
    }

    #[test]
    fn every_kernel_can_do_at_least_one_thing() {
        // A kernel that declares nothing is the stub this milestone replaced.
        // Registering one would offer a session that cannot be started.
        for (id, adapter) in registry(&Config::sample()) {
            let c = adapter.capabilities();
            assert!(
                c.conversation || c.terminal,
                "{id} offers no plane a session could run on"
            );
        }
    }
}
