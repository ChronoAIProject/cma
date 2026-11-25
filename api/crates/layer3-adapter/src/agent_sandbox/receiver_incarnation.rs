//! Fresh evidence from the exact owned Kubernetes runtime container.
use super::{model, rejected, AgentSandboxClient, PodBinding};
use crate::{Result, RuntimeHandle};
use kube::ResourceExt;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ReceiverIncarnation {
    pub(crate) image_id: String,
    started_at: String,
    runtime_generation: Option<u64>,
    sandbox_generation: Option<i64>,
}

impl ReceiverIncarnation {
    pub(crate) fn matches_retained_startup_policy(
        &self,
        readback: &crate::RetainedStartupPolicyReadback<'_>,
    ) -> bool {
        self.runtime_generation == Some(readback.runtime.generation)
            && crate::runtime_startup_policy::running_image_matches(
                &self.image_id,
                readback.image,
                readback.config_digest,
            )
    }
}

impl AgentSandboxClient {
    pub(crate) async fn receiver_incarnation(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
    ) -> Result<ReceiverIncarnation> {
        let (sandbox, pod) = self.ready_runtime_pod(runtime).await?;
        if model::pod_binding(&sandbox, &pod)? != *expected {
            return Err(rejected("runtime receiver incarnation changed"));
        }
        let invalid = || rejected("runtime receiver incarnation is incomplete");
        let container_id = expected
            .runtime_container_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .ok_or_else(invalid)?;
        expected
            .runtime_restart_count
            .filter(|count| *count >= 0)
            .ok_or_else(invalid)?;
        // Both native HTTP routes reach the Pod's network namespace. A second
        // process from a sidecar or host network would invalidate attribution.
        let spec = pod.spec.as_ref().ok_or_else(invalid)?;
        let status = pod.status.as_ref().ok_or_else(invalid)?;
        if spec.host_network == Some(true)
            || spec.containers.len() != 1
            || spec.containers[0].name != "runtime"
            || spec
                .ephemeral_containers
                .as_ref()
                .is_some_and(|items| !items.is_empty())
            || status
                .ephemeral_container_statuses
                .as_ref()
                .is_some_and(|items| !items.is_empty())
            || spec
                .init_containers
                .iter()
                .flatten()
                .any(|init| init.restart_policy.as_deref() == Some("Always"))
        {
            return Err(invalid());
        }
        let Some([container]) = status.container_statuses.as_deref() else {
            return Err(invalid());
        };
        if container.name != "runtime"
            || !container.ready
            || container.container_id.as_deref() != Some(container_id)
            || container.image_id.is_empty()
        {
            return Err(invalid());
        }
        let started_at = container
            .state
            .as_ref()
            .and_then(|state| state.running.as_ref())
            .and_then(|state| state.started_at.as_ref())
            .ok_or_else(invalid)?;
        let runtime_generation = sandbox
            .annotations()
            .get("cma/runtime-generation")
            .map(|value| {
                value
                    .parse::<u64>()
                    .ok()
                    .filter(|generation| *generation > 0)
                    .ok_or_else(invalid)
            })
            .transpose()?;
        Ok(ReceiverIncarnation {
            image_id: container.image_id.clone(),
            started_at: started_at.0.to_string(),
            runtime_generation,
            sandbox_generation: sandbox.metadata.generation,
        })
    }
}
