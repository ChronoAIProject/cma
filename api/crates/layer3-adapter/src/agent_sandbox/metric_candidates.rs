//! Physical infrastructure diagnostics. These observations grant no owner access.
use super::{api_error, metric_observation::quantity, model, rejected, AgentSandboxClient};
use crate::Result;
use futures_util::{stream, FutureExt, StreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::{api::ListParams, ResourceExt};
#[derive(Clone, Debug)]
pub struct MetricCandidate {
    pub namespace: String,
    pub deployment: String,
    pub native_uid: String,
    pub controller_uid: String,
    pub pod: super::PodBinding,
    pub pvc_name: String,
    pub pvc_uid: String,
    pub unassigned_pool: bool,
    pub cpu_limit: Option<f64>,
    pub memory_limit: Option<f64>,
}
pub struct MetricCandidates {
    pub items: Vec<MetricCandidate>,
    pub next: Option<String>,
    pub unproven: u64,
}
impl AgentSandboxClient {
    pub async fn metric_candidates(&self, after: Option<&str>) -> Result<MetricCandidates> {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut params = ListParams::default().labels(&self.selector()).limit(100);
            if let Some(after) = after {
                params = params.continue_token(after);
            }
            let page = self.pods.list(&params).await.map_err(api_error)?;
            let mut result = MetricCandidates {
                items: Vec::new(),
                next: page.metadata.continue_.filter(|v| !v.is_empty()),
                unproven: 0,
            };
            let futures: Vec<_> = page
                .items
                .into_iter()
                .map(|pod| {
                    let client = self.clone();
                    async move { client.metric_candidate(&pod).await }.boxed()
                })
                .collect();
            let mut proofs = stream::iter(futures).buffer_unordered(8);
            while let Some(proof) = proofs.next().await {
                match proof {
                    Ok(v) => result.items.push(v),
                    Err(_) => result.unproven += 1,
                }
            }
            Ok(result)
        })
        .await
        .map_err(|_| super::unavailable("metric candidate observation timed out"))?
    }
    async fn metric_candidate(&self, pod: &Pod) -> Result<MetricCandidate> {
        let refs = pod.metadata.owner_references.as_deref().unwrap_or_default();
        let owner = refs
            .first()
            .filter(|o| {
                refs.len() == 1
                    && o.kind == "Sandbox"
                    && o.api_version == "agents.x-k8s.io/v1beta1"
                    && o.controller == Some(true)
            })
            .ok_or_else(|| rejected("metric Pod native controller absent"))?;
        let member = self.sandboxes.get(&owner.name).await.map_err(api_error)?;
        if member.uid().as_deref() != Some(&owner.uid)
            || !model::owned_pod(pod, &self.config, &owner.name, &owner.uid)
        {
            return Err(rejected("metric native Pod identity changed"));
        }
        let (unassigned_pool, controller_uid) = self.pools.metric_controller(&member).await?;
        let spec = pod
            .spec
            .as_ref()
            .ok_or_else(|| rejected("metric Pod spec absent"))?;
        if spec.containers.len() != 1 || spec.containers[0].name != "runtime" {
            return Err(rejected("unsupported metric container"));
        }
        let pvc_name = spec
            .volumes
            .as_ref()
            .and_then(|vs| vs.iter().find(|v| v.name == "home"))
            .and_then(|v| v.persistent_volume_claim.as_ref())
            .map(|v| v.claim_name.clone())
            .ok_or_else(|| rejected("metric home absent"))?;
        let volume = self.claims.get(&pvc_name).await.map_err(api_error)?;
        let refs = volume
            .metadata
            .owner_references
            .as_deref()
            .unwrap_or_default();
        if volume.namespace().as_deref() != Some(&self.config.namespace)
            || refs.len() != 1
            || !refs.iter().all(|o| {
                o.kind == "Sandbox"
                    && o.api_version == "agents.x-k8s.io/v1beta1"
                    && o.uid == owner.uid
                    && o.name == owner.name
                    && o.controller == Some(true)
            })
        {
            return Err(rejected("metric PVC native controller absent"));
        }
        let pvc_uid = volume
            .uid()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| rejected("metric PVC UID absent"))?;
        let binding = model::pod_binding(&member, pod)?;
        if binding
            .runtime_container_id
            .as_deref()
            .is_none_or(str::is_empty)
            || binding.runtime_restart_count.is_none()
        {
            return Err(rejected("metric container incarnation absent"));
        }
        // Names and annotations never supply a CMA Sandbox ID or owner here.
        let limits = spec.containers[0]
            .resources
            .as_ref()
            .and_then(|r| r.limits.as_ref());
        Ok(MetricCandidate {
            namespace: self.config.namespace.clone(),
            deployment: self.config.deployment.clone(),
            native_uid: owner.uid.clone(),
            controller_uid,
            pod: binding,
            pvc_name,
            pvc_uid,
            unassigned_pool,
            cpu_limit: limits
                .and_then(|l| l.get("cpu"))
                .and_then(|v| quantity(&v.0)),
            memory_limit: limits
                .and_then(|l| l.get("memory"))
                .and_then(|v| quantity(&v.0)),
        })
    }
}
