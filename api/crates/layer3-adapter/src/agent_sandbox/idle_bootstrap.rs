//! Evidence for the legacy bootstrap that has not entered supervisor startup.
use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;

use super::{nonexecuting::validated_runtime_container, rejected, AgentSandboxClient};
use crate::{
    hosted::{IdleBootstrapProof, RuntimeRecoveryCommand},
    provider::ProviderSandboxRequest,
    HomeVolumeAttachment, Result,
};

// The old binary has no read-only bootstrap inspection method. Support only this
// verified immutable image/binary pair, with its permanent supervisor-reaper rule.
const IMAGE: &str = "@sha256:3c5bccfabe8d20bd1fb5900cbfb2958878a316d75cfd1d186bcbfcf91c6fbf28";
const BINARY: &str = "5a39d8c147fb9061e85831aa5fc7f42c405374719c4bc6d040e94dd2f997939b";

impl AgentSandboxClient {
    pub(super) async fn inspect_idle_bootstrap(
        &self,
        pod: &Pod,
        member: &kube::api::DynamicObject,
        home: &HomeVolumeAttachment,
        request: &ProviderSandboxRequest,
    ) -> Result<Option<(String, i32, IdleBootstrapProof)>> {
        if !request.image.uri.ends_with(IMAGE) {
            return Ok(None);
        }
        let Some(container) = validated_runtime_container(pod, member, home, request)? else {
            return Ok(None);
        };
        let Some(state) = &container.state else {
            return Ok(None);
        };
        if state
            .running
            .as_ref()
            .is_none_or(|state| state.started_at.is_none())
            || state.waiting.is_some()
            || state.terminated.is_some()
            || container.restart_count <= 0
            || container
                .last_state
                .as_ref()
                .and_then(|s| s.terminated.as_ref())
                .is_none_or(|last| {
                    last.reason.as_deref() != Some("Error")
                        || last.exit_code != 2
                        || last.finished_at.is_none()
                })
        {
            return Ok(None);
        }
        let id = container
            .container_id
            .as_ref()
            .filter(|id| !id.is_empty() && id.len() <= 512)
            .ok_or_else(|| rejected("idle runtime container identity is absent"))?;
        let command = RuntimeRecoveryCommand {
            program: "/usr/bin/python3".into(),
            args: vec![
                "-I".into(),
                "-c".into(),
                include_str!("idle_bootstrap.py").into(),
            ],
            input: vec![],
        };
        let output = self.inspect_pod(&pod.name_any(), &command).await?;
        let value: serde_json::Value = serde_json::from_slice(&output)
            .map_err(|_| rejected("idle bootstrap inspection returned invalid evidence"))?;
        if value["status"] != "idle" {
            return Ok(None);
        }
        let proof: IdleBootstrapProof = serde_json::from_value(value["proof"].clone())
            .map_err(|_| rejected("idle bootstrap process proof is incomplete"))?;
        if proof.version != 1
            || proof.pid_start_ticks == 0
            || proof.binary_sha256 != BINARY
            || proof.process_sha256.len() != 64
            || !proof
                .process_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(rejected("idle bootstrap process proof is invalid"));
        }
        Ok(Some((id.clone(), container.restart_count, proof)))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn idle_bootstrap_requires_complete_process_descriptor_and_thread_proof() {
        let output = std::process::Command::new("python3")
            .arg("-B")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/agent_sandbox/idle_bootstrap_tests.py"
            ))
            .output()
            .expect("Python is required for pinned runtime inspection tests");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
