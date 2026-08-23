//! Which kernels this deployment runs, and what it configures them with.
//!
//! The composition root's one job that is worth its own file: this is the only
//! place in the service where a concrete kernel is named. Everything upstream
//! selects by opaque identifier, which is what lets a second kernel arrive as a
//! directory in Layer 3 and a block here rather than as a change to anything
//! that uses one.

use std::collections::HashMap;
use std::sync::Arc;

use layer2_core::KernelId;
use layer3_adapter::{
    claude_code::{ClaudeCodeAdapter, ClaudeCodeSandboxSpec},
    codex::{CodexAdapter, CodexSandboxSpec},
    dsh::{DshAdapter, DshSandboxSpec},
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
    // second kernel arrive as a directory in Layer 3 and a block here.
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

    // The request shape this one speaks is its own rather than a deployment's
    // choice, so nothing here touches it.
    let mut claude_code = ClaudeCodeSandboxSpec::default();
    deploy(config, &mut claude_code.deployment);
    claude_code.deployment.upstream_base_url = config.claude_upstream_base_url.clone();
    claude_code.deployment.upstream_model = config.claude_upstream_model.clone();
    adapters.insert(
        KernelId::new("claude_code"),
        Arc::new(ClaudeCodeAdapter::new(
            SandboxClient::new(
                config.sandbox_base_url.clone(),
                config.sandbox_api_key.clone(),
            ),
            claude_code,
        )),
    );

    // The request shape this one speaks is its own too, and its route is
    // declared in a profile the image bakes rather than passed at startup.
    let mut dsh = DshSandboxSpec::default();
    deploy(config, &mut dsh.deployment);
    dsh.deployment.upstream_base_url = config.dsh_upstream_base_url.clone();
    dsh.deployment.upstream_model = config.dsh_upstream_model.clone();
    adapters.insert(
        KernelId::new("dsh"),
        Arc::new(DshAdapter::new(
            SandboxClient::new(
                config.sandbox_base_url.clone(),
                config.sandbox_api_key.clone(),
            ),
            dsh,
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
/// The sandbox's default size and image, the identity platform, and what the
/// image was built carrying are facts about this deployment rather than about
/// an agent, so they are stated once and every kernel gets the same ones.
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
    // What the image is built with. Skills are deliberately absent from this:
    // they used to be assigned the very list they were checked against, so a
    // skill could never be found missing — and none was ever installed. They
    // are delivered now, so where each kernel reads them is that kernel's own
    // business and is set in its own directory.
    deployment.provided_packages = config.baseline_packages.clone();
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
        let kernels = [
            CodexSandboxSpec::default().deployment,
            ClaudeCodeSandboxSpec::default().deployment,
            DshSandboxSpec::default().deployment,
        ];
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
    fn every_kernel_is_deployed_into_the_same_sandbox() {
        // The image, the baseline and the deployment's default size are facts
        // about this deployment rather than about an agent, so a kernel that
        // got different ones would be a session whose cost and promises
        // depended on which agent it picked.
        //
        // Per-session sizing moves this rather than retiring it. A session's
        // size may now vary by owner, and must still never vary by kernel. What
        // `deploy` writes here is the floor a session that states no size falls
        // back to, and all three kernels must fall back to the same one.
        let config = deployment_config();
        let mut deployments = [
            CodexSandboxSpec::default().deployment,
            ClaudeCodeSandboxSpec::default().deployment,
            DshSandboxSpec::default().deployment,
        ];
        for deployment in &mut deployments {
            deploy(&config, deployment);
        }

        for pair in deployments.windows(2) {
            assert_eq!(pair[0].shape.image, pair[1].shape.image);
            assert_eq!(pair[0].shape.resource_limits, pair[1].shape.resource_limits);
            assert_eq!(
                pair[0].shape.telemetry_interval_ms,
                pair[1].shape.telemetry_interval_ms
            );
            assert_eq!(pair[0].provided_packages, pair[1].provided_packages);
        }
    }

    #[test]
    fn only_codex_declares_a_standing_instructions_file() {
        // Shipping the setting before the other kernels read an instruction
        // file was an explicit product decision. An empty path is the adapter's
        // positive statement that no file is written for that kernel.
        let mut codex = CodexSandboxSpec::default().deployment;
        let mut claude_code = ClaudeCodeSandboxSpec::default().deployment;
        let mut dsh = DshSandboxSpec::default().deployment;
        for deployment in [&mut codex, &mut claude_code, &mut dsh] {
            deploy(&deployment_config(), deployment);
        }

        assert_eq!(codex.agent_notes, "AGENTS.md");
        assert!(claude_code.agent_notes.is_empty());
        assert!(dsh.agent_notes.is_empty());
    }

    #[test]
    fn no_two_kernels_share_a_bridge() {
        // If they did, one kernel's sandbox would run the other's program, and
        // nothing would say so until a session failed to answer.
        let commands = [
            CodexSandboxSpec::default().deployment.bridge_command,
            ClaudeCodeSandboxSpec::default().deployment.bridge_command,
            DshSandboxSpec::default().deployment.bridge_command,
        ];
        let mut unique = commands.clone().to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), commands.len(), "got {commands:?}");
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
