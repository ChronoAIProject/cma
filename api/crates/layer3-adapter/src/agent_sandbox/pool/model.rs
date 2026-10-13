use kube::{api::DynamicObject, ResourceExt};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{rejected, AgentSandboxConfig, Result, API_VERSION, DIGEST_ANNOTATION, OWNER_LABEL};
use crate::{HomeVolumePolicy, HomeVolumeRequest, RuntimeImage};

/// A native pool is defined by an immutable image, resources and initialized disk.
/// User-facing profiles and tenant runtime configuration never enter its template.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolSpec {
    pub image: RuntimeImage,
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub home: HomeVolumePolicy,
}

impl NativePoolSpec {
    pub fn validate(&self) -> Result<()> {
        crate::runtime_bootstrap::admit_pool_image(&self.image)?;
        self.home.validate()?;
        if serde_json::to_vec(self)
            .map_err(|_| rejected("invalid native pool configuration"))?
            .len()
            > 64 * 1024
        {
            return Err(rejected("native pool configuration exceeds its bound"));
        }
        let Some((name, digest)) = self.image.uri.rsplit_once("@sha256:") else {
            return Err(rejected("pool image must be pinned by digest"));
        };
        if name.is_empty()
            || !crate::provider_finality::digest(digest)
            || self.image.uri.len() > 2048
            || self.image.uri.chars().any(char::is_whitespace)
            || !(1..=1_048_576).contains(&self.cpu_millis)
            || !(1..=1_048_576).contains(&self.memory_mib)
            || !super::super::model::dns_label(&self.home.storage_class)
            || (!self.home.is_native_vct()
                && !super::super::model::dns_label(&self.home.template_name))
        {
            return Err(rejected("invalid native warm pool configuration"));
        }
        Ok(())
    }
}

/// Persist before creating a claim. Replacement pool/template UIDs cannot inherit it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolBinding {
    pub spec: NativePoolSpec,
    pub pod_policy: NativePoolPodPolicy,
    pub name: String,
    pub uid: String,
    pub template_name: String,
    pub template_uid: String,
    pub config_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolPodPolicy {
    pub runtime_class: Option<String>,
    pub service_account: String,
    pub image_pull_secrets: Vec<String>,
}

impl From<&AgentSandboxConfig> for NativePoolPodPolicy {
    fn from(config: &AgentSandboxConfig) -> Self {
        Self {
            runtime_class: config.runtime_class.clone(),
            service_account: config.service_account.clone(),
            image_pull_secrets: config.image_pull_secrets.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolStatus {
    pub binding: NativePoolBinding,
    pub desired_replicas: u32,
    /// Zero until the controller reports the current pool generation.
    pub ready_replicas: u32,
    pub observed: bool,
}

/// Durable authority for one logical Sandbox/generation, recorded before claim I/O.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolClaimRequest {
    pub pool: NativePoolBinding,
    pub home: HomeVolumeRequest,
    pub runtime_generation: u64,
}

impl NativePoolClaimRequest {
    pub fn validate(&self) -> Result<()> {
        self.pool.spec.validate()?;
        self.home.validate()?;
        if self.runtime_generation == 0
            || self.home.policy != self.pool.spec.home
            || !crate::provider_finality::digest(&self.pool.config_sha256)
            || !super::super::model::dns_label(&self.pool.pod_policy.service_account)
            || self
                .pool
                .pod_policy
                .runtime_class
                .as_ref()
                .is_some_and(|v| !super::super::model::dns_label(v))
            || self
                .pool
                .pod_policy
                .image_pull_secrets
                .iter()
                .any(|v| !super::super::model::dns_label(v))
            || [
                &self.pool.name,
                &self.pool.uid,
                &self.pool.template_name,
                &self.pool.template_uid,
            ]
            .into_iter()
            .any(|v| !super::super::model::dns_label(v))
        {
            return Err(rejected("invalid native pool claim authority"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolClaim {
    pub request: NativePoolClaimRequest,
    pub name: String,
    pub uid: String,
    pub resource_version: String,
    pub sandbox_name: Option<String>,
}

pub(super) fn digest(config: &AgentSandboxConfig, spec: &NativePoolSpec) -> Result<String> {
    config.validate()?;
    spec.validate()?;
    // Rotation of the deployment-only HMAC secret does not replace pool Pods.
    crate::provider_request_digest(&(
        "native-pool-v1",
        &config.namespace,
        &config.deployment,
        &config.runtime_class,
        &config.service_account,
        &config.image_pull_secrets,
        spec,
    ))
}

pub(super) fn template(
    config: &AgentSandboxConfig,
    spec: &NativePoolSpec,
) -> Result<DynamicObject> {
    let hash = digest(config, spec)?;
    let name = format!("cma-template-{}", &hash[..48]);
    let labels = json!({OWNER_LABEL:config.deployment,"cma/pool-config":&hash[..52]});
    let mounts: Vec<_> = [
        ("/home/cma", "home"),
        ("/var/lib/cma-runtime", "state"),
        ("/var/lib/cma-transfer", "transfer"),
    ]
    .into_iter()
    .map(|(path, sub)| json!({"name":"home","mountPath":path,"subPath":sub,"readOnly":false}))
    .collect();
    let pulls: Vec<_> = config
        .image_pull_secrets
        .iter()
        .map(|name| json!({"name":name}))
        .collect();
    let resources =
        json!({"cpu":format!("{}m",spec.cpu_millis),"memory":format!("{}Mi",spec.memory_mib)});
    let mut pod = json!({
        "metadata":{"labels":labels},"spec":{
            "automountServiceAccountToken":false,"serviceAccountName":config.service_account,
            "restartPolicy":"Always","imagePullSecrets":pulls,
            "hostNetwork":false,"hostPID":false,"hostIPC":false,"shareProcessNamespace":false,
            "initContainers":[],"ephemeralContainers":[],
            "securityContext":{"runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,"fsGroup":10001},
            "containers":[{"name":"runtime","image":spec.image.uri,
                "command":["/usr/local/bin/layer4-runtime","--wait-for-assignment"],"args":[],
                "env":[{"name":"CHRONO_RUNTIME_POD_UID","valueFrom":{"fieldRef":{"apiVersion":"v1","fieldPath":"metadata.uid"}}}],
                "envFrom":[],"resources":{"requests":resources,"limits":resources},
                "ports":[{"name":"control","containerPort":9090}],
                "readinessProbe":{"exec":{"command":["/usr/bin/test","-S",crate::runtime_bootstrap::RUNTIME_BOOTSTRAP_SOCKET]},"periodSeconds":2},
                "securityContext":{"allowPrivilegeEscalation":false,"privileged":false,
                    "runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,
                    "seccompProfile":{"type":"RuntimeDefault"},"capabilities":{"drop":["ALL"],"add":[]}},
                "volumeDevices":[],
                "volumeMounts":mounts}]
        }
    });
    if let Some(class) = &config.runtime_class {
        pod["spec"]["runtimeClassName"] = json!(class);
    }
    let mut template = json!({"apiVersion":API_VERSION,"kind":"SandboxTemplate",
        "metadata":{"name":name,"namespace":config.namespace,"labels":labels,
            "annotations":{DIGEST_ANNOTATION:hash}},
        "spec":{"service":false,"podTemplate":pod,"volumeClaimTemplatesPolicy":"Disallowed",
            "volumeClaimTemplates":[{"metadata":{"name":"home","labels":{"cma/pool-config":&hash[..52]},
                "annotations":{"cma/pool-seed-uid":spec.home.template_generation}},
                "spec":{"accessModes":["ReadWriteOncePod"],"volumeMode":"Filesystem",
                    "storageClassName":spec.home.storage_class,
                    "resources":{"requests":{"storage":format!("{}Gi",spec.home.capacity_gib)}},
                    "dataSource":{"apiGroup":"snapshot.storage.k8s.io","kind":"VolumeSnapshot","name":spec.home.template_name}}}]}});
    if spec.home.is_native_vct() {
        template["spec"]["volumeClaimTemplates"][0]["spec"]
            .as_object_mut()
            .unwrap()
            .remove("dataSource");
        template["spec"]["volumeClaimTemplates"][0]["metadata"]["annotations"] =
            json!({"cma/home-layout":"native-vct-empty-v1"});
        template["spec"]["podTemplate"]["spec"]["initContainers"] = json!([{
            "name":"initialize-home","image":spec.image.uri,
            "command":["/usr/local/bin/layer4-runtime","--initialize-home","/mnt/cma-home"],
            "args":[],"env":[],"envFrom":[],
            "securityContext":{"allowPrivilegeEscalation":false,"privileged":false,
                "runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,
                "seccompProfile":{"type":"RuntimeDefault"},"capabilities":{"drop":["ALL"]}},
            "resources":{"requests":{"cpu":"10m","memory":"16Mi"},"limits":{"cpu":"100m","memory":"64Mi"}},
            "volumeMounts":[{"name":"home","mountPath":"/mnt/cma-home","readOnly":false}]
        }]);
    }
    decode(template)
}

pub(super) fn pool(
    config: &AgentSandboxConfig,
    spec: &NativePoolSpec,
    template: &DynamicObject,
    replicas: u32,
) -> Result<DynamicObject> {
    if replicas > 1000 {
        return Err(rejected("native warm pool target exceeds its bound"));
    }
    let hash = digest(config, spec)?;
    decode(json!({"apiVersion":API_VERSION,"kind":"SandboxWarmPool",
        "metadata":{"name":format!("cma-pool-{}",&hash[..48]),"namespace":config.namespace,
            "labels":{OWNER_LABEL:config.deployment},
            "annotations":{DIGEST_ANNOTATION:hash,"cma/pool-template-uid":template.uid(),
                super::DEFINITION_ANNOTATION:super::management::definition(config,spec)?}},
        "spec":{"replicas":replicas,"sandboxTemplateRef":{"name":template.name_any()},
            "updateStrategy":{"type":"OnReplenish"}}}))
}

pub(super) fn claim(
    config: &AgentSandboxConfig,
    request: &NativePoolClaimRequest,
) -> Result<DynamicObject> {
    request.validate()?;
    let original_config = binding_config(config, &request.pool);
    if request.pool.config_sha256 != digest(&original_config, &request.pool.spec)?
        || request.pool.name != format!("cma-pool-{}", &request.pool.config_sha256[..48])
        || request.pool.template_name
            != format!("cma-template-{}", &request.pool.config_sha256[..48])
    {
        return Err(rejected(
            "native pool binding belongs to another configuration",
        ));
    }
    let name = format!(
        "cma-claim-{}",
        &crate::provider_request_digest(&(
            &config.deployment,
            &request.home.sandbox_id,
            request.runtime_generation,
        ))?[..48]
    );
    decode(json!({"apiVersion":API_VERSION,"kind":"SandboxClaim",
        "metadata":{"name":name,"namespace":config.namespace,"labels":{OWNER_LABEL:config.deployment},
            "annotations":{DIGEST_ANNOTATION:crate::provider_request_digest(request)?,
                "cma/pool-claim-request":serde_json::to_string(request).map_err(|_|rejected("invalid pool claim request"))?}},
        // env or claim-level VCTs bypass upstream warm allocation. Neither belongs here.
        "spec":{"warmPoolRef":{"name":request.pool.name}}}))
}

pub(super) fn binding_config(
    current: &AgentSandboxConfig,
    binding: &NativePoolBinding,
) -> AgentSandboxConfig {
    // A deployment setting change may affect new pools; existing reservations
    // retain the exact service account, runtime class and image pull policy.
    AgentSandboxConfig {
        runtime_class: binding.pod_policy.runtime_class.clone(),
        service_account: binding.pod_policy.service_account.clone(),
        image_pull_secrets: binding.pod_policy.image_pull_secrets.clone(),
        ..current.clone()
    }
}

pub(super) fn verify(actual: &DynamicObject, desired: &DynamicObject) -> Result<()> {
    if actual.name_any() != desired.name_any()
        || actual.namespace() != desired.namespace()
        || actual.types != desired.types
        || actual.metadata.deletion_timestamp.is_some()
        || actual.uid().as_deref().is_none_or(str::is_empty)
        || actual
            .resource_version()
            .as_deref()
            .is_none_or(str::is_empty)
        || actual
            .metadata
            .owner_references
            .as_ref()
            .is_some_and(|owners| !owners.is_empty())
        || desired
            .labels()
            .iter()
            .any(|(k, v)| actual.labels().get(k) != Some(v))
        || desired
            .annotations()
            .iter()
            .any(|(k, v)| actual.annotations().get(k) != Some(v))
        || !contains(&actual.data["spec"], &desired.data["spec"])
    {
        return Err(rejected(
            "native pool resource conflicts with its recorded configuration",
        ));
    }
    Ok(())
}

// API defaults may add ordinary fields; all submitted values and array cardinality stay exact.
pub(crate) fn contains(actual: &serde_json::Value, desired: &serde_json::Value) -> bool {
    match desired {
        serde_json::Value::Object(fields) => actual.as_object().is_some_and(|a| {
            fields.iter().all(|(k, v)| {
                a.get(k).map_or_else(
                    || v.as_array().is_some_and(Vec::is_empty),
                    |x| contains(x, v),
                )
            })
        }),
        serde_json::Value::Array(values) => actual.as_array().is_some_and(|a| {
            a.len() == values.len() && a.iter().zip(values).all(|(x, v)| contains(x, v))
        }),
        _ => actual == desired,
    }
}

fn decode(value: serde_json::Value) -> Result<DynamicObject> {
    serde_json::from_value(value).map_err(|_| rejected("invalid native pool resource"))
}
