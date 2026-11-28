use super::{rejected, unavailable, OWNER_LABEL, RUNTIME_LABEL, SPEC_ANNOTATION};
use crate::{provider::ProviderSandboxRequest, Result, RuntimeHandle};
use k8s_openapi::api::core::v1::Pod;
use kube::{api::DynamicObject, ResourceExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq)]
pub struct AgentSandboxConfig {
    pub namespace: String,
    pub deployment: String,
    pub runtime_class: Option<String>,
    pub service_account: String,
    pub image_pull_secrets: Vec<String>,
    pub template_network_policy: Option<super::NativeTemplateNetworkPolicy>,
    pub template_fs_group_change_policy: Option<super::NativeFsGroupChangePolicy>,
    pub native_claim_guard_enabled: bool,
    pub runtime_startup_breaker_enabled: bool,
    pub control_key: String,
}

impl std::fmt::Debug for AgentSandboxConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSandboxConfig")
            .field("namespace", &self.namespace)
            .field("deployment", &self.deployment)
            .field("runtime_class", &self.runtime_class)
            .field("service_account", &self.service_account)
            .field("image_pull_secrets", &self.image_pull_secrets)
            .field("template_network_policy", &self.template_network_policy)
            .field(
                "template_fs_group_change_policy",
                &self.template_fs_group_change_policy,
            )
            .field("control_key", &"<redacted>")
            .field(
                "native_claim_guard_enabled",
                &self.native_claim_guard_enabled,
            )
            .field(
                "runtime_startup_breaker_enabled",
                &self.runtime_startup_breaker_enabled,
            )
            .finish()
    }
}

impl AgentSandboxConfig {
    pub fn validate(&self) -> Result<()> {
        if let Some(policy) = &self.template_network_policy {
            policy.validate()?;
        }
        if !dns_label(&self.namespace)
            || !label_value(&self.deployment)
            || !dns_label(&self.service_account)
            || self.runtime_class.as_ref().is_some_and(|s| !dns_label(s))
            || self.image_pull_secrets.iter().any(|s| !dns_label(s))
            || self.control_key.len() < 32
            || self.control_key.len() > 8192
        {
            return Err(rejected("invalid agent-sandbox deployment configuration"));
        }
        Ok(())
    }
}

pub(super) fn dns_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
}

fn label_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
}

pub(crate) fn handle(object: &DynamicObject) -> Result<RuntimeHandle> {
    let name = object.name_any();
    let uid = object
        .uid()
        .ok_or_else(|| rejected("Sandbox UID missing"))?;
    let value = RuntimeHandle(format!("agent-sandbox:{name}:{uid}"));
    parse_handle(&value)?;
    Ok(value)
}

pub(crate) fn parse_handle(value: &RuntimeHandle) -> Result<(&str, &str)> {
    let (name, uid) = value
        .0
        .strip_prefix("agent-sandbox:")
        .and_then(|s| s.split_once(':'))
        .ok_or_else(|| rejected("invalid agent-sandbox runtime handle"))?;
    if !dns_label(name) || !dns_label(uid) {
        return Err(rejected("invalid Sandbox name or UID"));
    }
    Ok((name, uid))
}

pub(super) fn name(request: &ProviderSandboxRequest) -> Result<String> {
    let identity: BTreeMap<_, _> = [OWNER_LABEL, "cma/sandbox", "cma/runtime-generation"]
        .into_iter()
        .map(|key| {
            request
                .metadata
                .get(key)
                .map(|v| (key, v))
                .ok_or_else(|| rejected("CMA runtime identity is missing"))
        })
        .collect::<Result<_>>()?;
    Ok(format!(
        "cma-{}",
        &crate::provider_request_digest(&identity)?[..56]
    ))
}

#[cfg(test)]
pub(super) fn desired(
    config: &AgentSandboxConfig,
    request: &ProviderSandboxRequest,
    control_token: &str,
) -> Result<DynamicObject> {
    desired_at(config, request, control_token, &name(request)?)
}

pub(super) fn desired_at(
    config: &AgentSandboxConfig,
    request: &ProviderSandboxRequest,
    control_token: &str,
    name: &str,
) -> Result<DynamicObject> {
    config.validate()?;
    if !dns_label(name) {
        return Err(rejected("invalid retained Sandbox name"));
    }
    request.validate_volumes()?;
    if request.metadata.get(OWNER_LABEL) != Some(&config.deployment)
        || request.entrypoint.is_empty()
        || request.timeout.is_some()
        || request.env.contains_key("CHRONO_RUNTIME_CONTROL_TOKEN")
    {
        return Err(rejected(
            "Sandbox request has invalid ownership, command or lifecycle configuration",
        ));
    }
    let (_, digest) = request
        .image
        .uri
        .rsplit_once("@sha256:")
        .ok_or_else(|| rejected("runtime image must be pinned by digest"))?;
    if !crate::provider_finality::digest(digest) {
        return Err(rejected("invalid runtime image digest"));
    }
    let cpu = resource(request, "cpu", "m")?;
    let memory = resource(request, "memory", "Mi")?;
    let labels = json!({OWNER_LABEL: config.deployment, RUNTIME_LABEL: name});
    let env: Vec<_> = request
        .env
        .iter()
        .map(|(name, value)| json!({"name":name,"value":value}))
        .chain(std::iter::once(
            json!({"name":"CHRONO_RUNTIME_CONTROL_TOKEN", "value":control_token}),
        ))
        .collect();
    let volumes: Vec<_> = request.volumes.iter().map(|v| json!({"name":v.name,"persistentVolumeClaim":{"claimName":v.pvc.claim_name,"readOnly":v.read_only}})).collect();
    let mounts: Vec<_> = request
        .volumes
        .iter()
        .map(|v| {
            let mut mount = json!({"name":v.name,"mountPath":v.mount_path,"readOnly":v.read_only});
            if let Some(path) = &v.sub_path {
                mount["subPath"] = json!(path);
            }
            mount
        })
        .collect();
    let pull: Vec<_> = config
        .image_pull_secrets
        .iter()
        .map(|name| json!({"name":name}))
        .collect();
    let mut spec = json!({
        "service":false,"operatingMode":"Running",
        "podTemplate":{"metadata":{"labels":labels},"spec":{
            "automountServiceAccountToken":false,"serviceAccountName":config.service_account,
            "restartPolicy":"Always","imagePullSecrets":pull,
            "securityContext":{"runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,"fsGroup":10001},
            "containers":[{"name":"runtime","image":request.image.uri,"command":request.entrypoint,
                "env":env,"resources":{"requests":{"cpu":cpu,"memory":memory},"limits":{"cpu":cpu,"memory":memory}},
                "ports":[{"name":"control","containerPort":9090}],
                "readinessProbe":{"tcpSocket":{"port":9090},"periodSeconds":2},
                "securityContext":{"allowPrivilegeEscalation":false,"capabilities":{"drop":["ALL"]}},
                "volumeMounts":mounts}],"volumes":volumes}}
    });
    if let Some(class) = &config.runtime_class {
        spec["podTemplate"]["spec"]["runtimeClassName"] = json!(class);
    }
    let mut annotations = request.metadata.clone();
    annotations.insert(
        SPEC_ANNOTATION.into(),
        crate::provider_request_digest(&spec)?,
    );
    super::funding::apply(&mut spec, request.funded_until)?;
    serde_json::from_value(json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
        "metadata":{"name":name,"namespace":config.namespace,"labels":labels,"annotations":annotations},"spec":spec}))
        .map_err(|_| rejected("cannot encode Sandbox resource"))
}

fn resource<'a>(request: &'a ProviderSandboxRequest, key: &str, unit: &str) -> Result<&'a str> {
    let value = request
        .resource_limits
        .get(key)
        .ok_or_else(|| rejected("CPU and memory are required"))?;
    let quantity = value.strip_suffix(unit).and_then(|s| s.parse::<u64>().ok());
    if quantity.is_none_or(|n| n == 0 || n > 1_048_576) {
        return Err(rejected("invalid resource size"));
    }
    Ok(value)
}

pub(super) fn ready(object: &DynamicObject) -> bool {
    let generation = object.metadata.generation.unwrap_or(0);
    object
        .data
        .pointer("/status/conditions")
        .and_then(|v| v.as_array())
        .is_some_and(|rows| {
            rows.iter().any(|c| {
                c["type"] == "Ready"
                    && c["status"] == "True"
                    && generation > 0
                    && c["observedGeneration"].as_i64() == Some(generation)
            })
        })
}

pub(super) fn owned_pod(pod: &Pod, config: &AgentSandboxConfig, name: &str, uid: &str) -> bool {
    pod.namespace().as_deref() == Some(&config.namespace)
        && pod.labels().get(OWNER_LABEL) == Some(&config.deployment)
        && pod.labels().get(super::pods::TRACKING_LABEL) == Some(&super::pods::name_hash(name))
        && pod
            .metadata
            .owner_references
            .as_ref()
            .is_some_and(|owners| {
                owners.iter().any(|o| {
                    o.api_version == "agents.x-k8s.io/v1beta1"
                        && o.kind == "Sandbox"
                        && o.name == name
                        && o.uid == uid
                        && o.controller == Some(true)
                })
            })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodBinding {
    pub sandbox_name: String,
    pub sandbox_uid: String,
    pub pod_name: String,
    pub pod_uid: String,
    pub pod_ip: std::net::IpAddr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_container_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_restart_count: Option<i32>,
}

pub(super) fn pod_binding(object: &DynamicObject, pod: &Pod) -> Result<PodBinding> {
    let status = pod
        .status
        .as_ref()
        .ok_or_else(|| unavailable("Pod status missing"))?;
    if pod.metadata.deletion_timestamp.is_some()
        || status.phase.as_deref() != Some("Running")
        || !status
            .conditions
            .as_ref()
            .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
    {
        return Err(unavailable("Pod is not ready"));
    }
    let ip: std::net::IpAddr = status
        .pod_ip
        .as_ref()
        .and_then(|ip| ip.parse().ok())
        .ok_or_else(|| unavailable("Pod has no valid IP"))?;
    if ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || matches!(ip, std::net::IpAddr::V4(v) if v.is_link_local())
        || matches!(ip, std::net::IpAddr::V6(v) if v.is_unicast_link_local())
    {
        return Err(rejected("Pod address is not routable"));
    }
    let container = status.container_statuses.as_ref().and_then(|containers| {
        containers.iter().find(|container| {
            container.name == "runtime"
                && container
                    .state
                    .as_ref()
                    .is_some_and(|state| state.running.is_some())
        })
    });
    Ok(PodBinding {
        sandbox_name: object.name_any(),
        sandbox_uid: object
            .uid()
            .ok_or_else(|| rejected("Sandbox UID missing"))?,
        pod_name: pod.name_any(),
        pod_uid: pod
            .uid()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| rejected("Pod UID missing"))?,
        pod_ip: ip,
        runtime_container_id: container.and_then(|container| container.container_id.clone()),
        runtime_restart_count: container.map(|container| container.restart_count),
    })
}

/// Retirement may observe an owned Pod before it has a reachable endpoint.
pub(super) fn retirement_pod_binding(
    object: &DynamicObject,
    pod: &Pod,
) -> Result<Option<PodBinding>> {
    let status = pod
        .status
        .as_ref()
        .ok_or_else(|| unavailable("Pod status missing"))?;
    if pod.metadata.deletion_timestamp.is_some() {
        return Err(unavailable("retirement Pod is being replaced or deleted"));
    }
    match status.phase.as_deref() {
        Some("Pending") => return Ok(None),
        Some("Running") => {}
        _ => return Err(unavailable("retirement Pod phase is unknown")),
    }
    let ready = status
        .conditions
        .as_ref()
        .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"));
    let crash_loop = status.container_statuses.iter().flatten().any(|container| {
        container.name == "runtime"
            && container
                .state
                .as_ref()
                .and_then(|state| state.waiting.as_ref())
                .and_then(|waiting| waiting.reason.as_deref())
                == Some("CrashLoopBackOff")
    });
    if !ready || crash_loop || status.pod_ip.as_deref().is_none_or(str::is_empty) {
        return Ok(None);
    }
    pod_binding(object, pod).map(Some)
}
