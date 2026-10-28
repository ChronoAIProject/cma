//! Bounded read-only proof for the initial, no-Agent recovery exception.
use std::time::Duration;

use k8s_openapi::api::core::v1::{ContainerStatus, Pod};
use kube::{api::ListParams, ResourceExt};
use sha2::{Digest, Sha256};

use super::{api_error, model, parse_handle, pods, rejected, unavailable, AgentSandboxClient};
use crate::{
    hosted::NonexecutingRuntimeProof, provider::ProviderSandboxRequest, AdapterError,
    HomeVolumeAttachment, Result, RuntimeHandle,
};

impl AgentSandboxClient {
    pub(crate) async fn suspend_nonexecuting_runtime(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        proof: &NonexecutingRuntimeProof,
    ) -> Result<()> {
        use kube::api::{Patch, PatchParams};
        use serde_json::json;
        let (name, uid) = parse_handle(runtime)?;
        let mut desired = self.desired_at(request, name).await?;
        // All request/home rendering happens before the final stopped-container
        // observation. Never re-read a newer object and silently adopt its RV.
        if self
            .inspect_nonexecuting_runtime(runtime, request, home)
            .await?
            .as_ref()
            != Some(proof)
        {
            return Err(rejected(
                "nonexecuting runtime evidence changed before suspension",
            ));
        }
        let patch = json!({"metadata": {"uid": uid,
            "resourceVersion": proof.sandbox_resource_version},
            "spec": {"operatingMode": "Suspended"}});
        self.sandboxes
            .patch(name, &PatchParams::default(), &Patch::Merge(patch))
            .await
            .map_err(api_error)?;
        desired.data["spec"]["operatingMode"] = json!("Suspended");
        self.wait_suspended(runtime, &desired).await
    }

    pub(crate) async fn inspect_nonexecuting_runtime(
        &self,
        runtime: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<NonexecutingRuntimeProof>> {
        let origin = self.native_runtime_home_origin(runtime, home)?;
        let (name, _) = parse_handle(runtime)?;
        let _permit = self
            .runtime_requests
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                AdapterError::AtCapacity(
                    "nonexecuting runtime inspection capacity is exhausted".into(),
                )
            })?;
        tokio::time::timeout(Duration::from_secs(30), async {
            let parameters = ListParams::default().labels(&pods::selector(name)).limit(3);
            let (member, page, policy) = tokio::join!(
                self.pools.prove_native_home(home),
                self.pods.list(&parameters),
                self.verify_native_runtime_policy(runtime, home),
            );
            let member = member?;
            let desired = self
                .pools
                .runtime_desired_for_member(origin, home, request, name, &member)?;
            self.matches(&member, &desired)?;
            self.complete_native_runtime_policy(home, &member, policy)?;
            // No readiness gate: this branch exists specifically for failed startup.
            if model::ready(&member)
                || member
                    .data
                    .pointer("/spec/operatingMode")
                    .and_then(|v| v.as_str())
                    == Some("Suspended")
            {
                return Ok(None);
            }
            let page = page.map_err(api_error)?;
            if page.metadata.continue_.is_some_and(|v| !v.is_empty()) {
                return Err(unavailable(
                    "nonexecuting runtime Pod inventory is incomplete",
                ));
            }
            let uid = member
                .uid()
                .ok_or_else(|| rejected("nonexecuting Sandbox UID missing"))?;
            let mut owned = page
                .items
                .iter()
                .filter(|pod| model::owned_pod(pod, &self.config, name, &uid));
            let Some(pod) = owned.next() else {
                return Ok(None);
            };
            if owned.next().is_some() {
                return Err(unavailable("nonexecuting runtime has multiple owned Pods"));
            }
            let Some((container_id, restart_count)) = stopped_runtime(pod, &member, home, request)?
            else {
                return Ok(None);
            };
            let proof = NonexecutingRuntimeProof {
                version: 1,
                handle: runtime.clone(),
                request_sha256: crate::provider_request_digest(request)?,
                home_identity: home.identity()?,
                sandbox_generation: member
                    .metadata
                    .generation
                    .filter(|n| *n > 0)
                    .ok_or_else(|| rejected("nonexecuting Sandbox generation missing"))?,
                sandbox_resource_version: identifier(member.resource_version())?,
                pod_name: identifier(pod.metadata.name.clone())?,
                pod_uid: identifier(pod.uid())?,
                pod_resource_version: identifier(pod.resource_version())?,
                runtime_container_id: container_id,
                runtime_restart_count: restart_count,
                // Hash the observed spec/status; never return environment values,
                // termination messages or other provider text in the proof.
                state_sha256: format!(
                    "{:x}",
                    Sha256::digest(
                        serde_json::to_vec(&(&pod.spec, &pod.status))
                            .map_err(|_| rejected("nonexecuting Pod cannot be encoded"))?
                    )
                ),
            };
            if serde_json::to_vec(&proof)
                .map_err(|_| rejected("nonexecuting proof cannot be encoded"))?
                .len()
                > 4096
            {
                return Err(rejected("nonexecuting proof exceeds its bound"));
            }
            Ok(Some(proof))
        })
        .await
        .map_err(|_| unavailable("nonexecuting runtime inspection timed out"))?
    }
}

fn identifier(value: Option<String>) -> Result<String> {
    value
        .filter(|value| {
            !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
        })
        .ok_or_else(|| rejected("nonexecuting provider identity missing"))
}

fn stopped_runtime(
    pod: &Pod,
    member: &kube::api::DynamicObject,
    home: &HomeVolumeAttachment,
    request: &ProviderSandboxRequest,
) -> Result<Option<(String, i32)>> {
    let Some(status) = &pod.status else {
        return Ok(None);
    };
    if pod.metadata.deletion_timestamp.is_some()
        || !matches!(status.phase.as_deref(), Some("Running" | "Failed"))
        || status.conditions.as_ref().is_some_and(|conditions| {
            conditions
                .iter()
                .any(|c| c.type_ == "Ready" && c.status == "True")
        })
    {
        return Ok(None);
    }
    let spec = pod
        .spec
        .as_ref()
        .ok_or_else(|| rejected("nonexecuting Pod spec missing"))?;
    if spec.containers.len() != 1
        || spec.containers[0].name != "runtime"
        || spec
            .ephemeral_containers
            .as_ref()
            .is_some_and(|v| !v.is_empty())
        || status
            .ephemeral_container_statuses
            .as_ref()
            .is_some_and(|v| !v.is_empty())
    {
        return Ok(None);
    }
    let expected: k8s_openapi::api::core::v1::Container = serde_json::from_value(
        member
            .data
            .pointer("/spec/podTemplate/spec/containers/0")
            .cloned()
            .ok_or_else(|| rejected("nonexecuting runtime container missing"))?,
    )
    .map_err(|_| rejected("nonexecuting runtime container invalid"))?;
    let actual = &spec.containers[0];
    if actual.image.as_deref() != Some(&request.image.uri)
        || !same_container(actual, &expected)
        || !spec.volumes.as_ref().is_some_and(|volumes| {
            volumes.iter().any(|volume| {
                volume.name == "home"
                    && volume
                        .persistent_volume_claim
                        .as_ref()
                        .is_some_and(|claim| {
                            claim.claim_name == home.receipt.storage_reference
                                && claim.read_only != Some(true)
                        })
            })
        })
    {
        return Err(rejected(
            "nonexecuting Pod differs from its original runtime or home",
        ));
    }
    let expected_init: Vec<k8s_openapi::api::core::v1::Container> = serde_json::from_value(
        member
            .data
            .pointer("/spec/podTemplate/spec/initContainers")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    )
    .map_err(|_| rejected("nonexecuting runtime initializer invalid"))?;
    let init = spec.init_containers.as_deref().unwrap_or_default();
    if init.len() != expected_init.len()
        || init
            .iter()
            .zip(&expected_init)
            .any(|(actual, expected)| !same_container(actual, expected))
    {
        return Err(rejected(
            "nonexecuting Pod initializer differs from original authority",
        ));
    }
    let init_status = status
        .init_container_statuses
        .as_deref()
        .unwrap_or_default();
    if init.len() != init_status.len()
        || init.iter().any(|container| {
            init_status
                .iter()
                .filter(|s| s.name == container.name)
                .count()
                != 1
                || !init_status
                    .iter()
                    .any(|s| s.name == container.name && completed_init(s))
        })
    {
        return Ok(None);
    }
    let containers = status.container_statuses.as_deref().unwrap_or_default();
    if containers.len() != 1 || containers[0].name != "runtime" {
        return Ok(None);
    }
    let container = &containers[0];
    if container.ready || container.started == Some(true) || container.restart_count < 0 {
        return Ok(None);
    }
    let Some(state) = &container.state else {
        return Ok(None);
    };
    if state.running.is_some() {
        return Ok(None);
    }
    let terminated = match (&state.waiting, &state.terminated) {
        (Some(waiting), None)
            if waiting.reason.as_deref() == Some("CrashLoopBackOff")
                && container.restart_count > 0 =>
        {
            container
                .last_state
                .as_ref()
                .filter(|s| s.running.is_none() && s.waiting.is_none())
                .and_then(|s| s.terminated.as_ref())
        }
        (None, Some(terminated)) => Some(terminated),
        _ => None,
    };
    let Some(terminated) = terminated else {
        return Ok(None);
    };
    if terminated.reason.as_deref() != Some("Error")
        || terminated.exit_code <= 0
        || terminated.signal.is_some_and(|signal| signal != 0)
        || terminated.started_at.is_none()
        || terminated.finished_at.is_none()
        || terminated.finished_at < terminated.started_at
    {
        return Ok(None);
    }
    let container_id = identifier(
        terminated
            .container_id
            .clone()
            .or_else(|| container.container_id.clone()),
    )?;
    if container
        .container_id
        .as_ref()
        .is_some_and(|id| id != &container_id)
    {
        return Err(rejected("nonexecuting runtime container identity changed"));
    }
    Ok(Some((container_id, container.restart_count)))
}

fn completed_init(status: &ContainerStatus) -> bool {
    status.state.as_ref().is_some_and(|state| {
        state.running.is_none()
            && state.waiting.is_none()
            && state.terminated.as_ref().is_some_and(|terminated| {
                terminated.exit_code == 0 && terminated.finished_at.is_some()
            })
    })
}

fn same_container(
    actual: &k8s_openapi::api::core::v1::Container,
    expected: &k8s_openapi::api::core::v1::Container,
) -> bool {
    let mounts = actual.volume_mounts.as_deref().unwrap_or_default();
    let expected_mounts = expected.volume_mounts.as_deref().unwrap_or_default();
    actual.name == expected.name
        && actual.image == expected.image
        && actual.command.as_deref().unwrap_or_default()
            == expected.command.as_deref().unwrap_or_default()
        && actual.args.as_deref().unwrap_or_default()
            == expected.args.as_deref().unwrap_or_default()
        && actual.env.as_deref().unwrap_or_default() == expected.env.as_deref().unwrap_or_default()
        && actual.env_from.as_deref().unwrap_or_default()
            == expected.env_from.as_deref().unwrap_or_default()
        && mounts.len() == expected_mounts.len()
        && mounts.iter().zip(expected_mounts).all(|(a, e)| {
            a.name == e.name
                && a.mount_path == e.mount_path
                && a.read_only.unwrap_or(false) == e.read_only.unwrap_or(false)
                && a.sub_path.as_deref().unwrap_or_default()
                    == e.sub_path.as_deref().unwrap_or_default()
                && a.sub_path_expr.as_deref().unwrap_or_default()
                    == e.sub_path_expr.as_deref().unwrap_or_default()
                && a.mount_propagation == e.mount_propagation
                && a.recursive_read_only == e.recursive_read_only
        })
}
